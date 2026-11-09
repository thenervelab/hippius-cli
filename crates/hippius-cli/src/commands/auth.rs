//! `hipc auth`: store, check, and remove the console API token, and log in with an access key.

use std::path::{Path, PathBuf};

use hippius::{
    AccessKey, Code, ConfigFile, Error, Given, Method, Overrides, Request, Result, SideEffects,
    Source, Token, phrase,
};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::commands::{Input, Printed, account, local_dry_run, replace};
use crate::context::{self, Context, Gate, Question, Step};
use crate::hidden::{self, Typed};
use crate::output;

/// `hipc auth login`: `--token-file <path>`, or at a terminal the token typed at a prompt,
/// stores a token the caller holds; `--access-key-file <path>`, or `--access-key` at a
/// terminal, logs in with an access key and stores the token the API answers. clap allows
/// only one of the three flags, and `new_account` only with an access key.
pub(crate) fn login(
    ctx: &Context<'_>,
    token_file: Option<&Path>,
    access_key_file: Option<&Path>,
    access_key: bool,
    new_account: bool,
) -> Result<Printed> {
    if access_key || access_key_file.is_some() {
        return login_with_access_key(ctx, access_key_file, new_account);
    }
    let token = match token_file {
        Some(path) => {
            let input = Input {
                path,
                flag: "--token-file",
                what: "token",
            };
            Token::new(&Zeroizing::new(input.read_secret()?), &input.source())?
        }
        None => asked_token(ctx)?,
    };
    let login = Login::new(ctx)?;
    // A different token stored in the profile is replaced, and every later command then acts
    // as whoever owns the new one: a token piped in from a hostile source would move the
    // user's work into another account, so the replacement is confirmed.
    let replaces = login.config.stores_other_token(&login.profile, &token);
    if ctx.dry_run() {
        return Ok(Printed::Document(local_dry_run(
            "store_token",
            login.dry_run(&token, replaces)?,
        )));
    }
    // A token bound to any origin but the default one is sent there from now on.
    ctx.confirm_origin(
        &login.target,
        "store the token for",
        "the token is then sent there and nowhere else",
    )?;
    if replaces {
        login.confirm_replace(ctx, "a different one")?;
    }
    let doc = login.store(ctx, &token)?;
    // `hipc auth status` checks `HIPPIUS_TOKEN` when it is set, not the token just stored,
    // so then no command checks it, and there is no note.
    if login.overrides.token.is_none()
        && let Some(command) = ctx.next_command("auth status")
    {
        ctx.note(&format!(
            "the token is stored, not checked; run `{command}` to check it"
        ));
    }
    Ok(Printed::Document(doc))
}

/// Where a login stores its token: the config file as it was read, its profile, the
/// overrides, and the origin, read once before the dry run and the questions, so both see what
/// the store does.
struct Login {
    path: PathBuf,
    config: ConfigFile,
    profile: String,
    overrides: Overrides,
    target: Given,
}

impl Login {
    fn new(ctx: &Context<'_>) -> Result<Self> {
        let path = context::config_path()?;
        let overrides = ctx.overrides()?;
        let config = ConfigFile::load(path.clone())?;
        let target = config.api_url(&overrides)?;
        // Parsed before `--dry-run`, so a dry run refuses the origin the real run refuses, with
        // a message that names where the origin came from.
        hippius::is_default_origin(&target)?;
        let profile = config.profile_name(&overrides)?;
        Ok(Self {
            path,
            config,
            profile,
            overrides,
            target,
        })
    }

    /// The fields of a dry run that would store `token`. This copy of the file is never
    /// written: storing the token in it only gives the origin as the real run stores it,
    /// normalized.
    fn dry_run(&self, token: &Token, replaces: bool) -> Result<Value> {
        let mut config = ConfigFile::load(self.path.clone())?;
        let origin = config.set_token(&self.profile, token, &self.target.value)?;
        Ok(json!({
            "profile": self.profile,
            "token_stored": true,
            "replaces_token": replaces,
            "token_origin": origin,
            "config_path": config.path().display().to_string(),
        }))
    }

    /// Asks before the token stored in the profile is replaced by `new`, which names the token
    /// that replaces it.
    fn confirm_replace(&self, ctx: &Context<'_>, new: &str) -> Result<()> {
        ctx.confirm(Question {
            action: &format!(
                "replace the token stored in the profile `{}` with {new}",
                self.profile
            ),
            consequence: Some(
                "every later command with this profile acts as the new token's owner",
            ),
        })
    }

    /// Stores `token` for the origin read before the questions, and returns the document a
    /// login prints.
    fn store(&self, ctx: &Context<'_>, token: &Token) -> Result<Value> {
        let profile = &self.profile;
        let origin = ConfigFile::edit(self.path.clone(), |now| {
            // The question was about the token stored when it was asked. One stored meanwhile
            // by another `hipc` was not confirmed, so it stays, and the run refuses.
            if !now.same_token(profile, &self.config) && now.stores_other_token(profile, token) {
                return Err(Error::usage(
                    format!(
                        "the stored token of the profile `{profile}` changed while the question \
                         waited, so it was left in place"
                    ),
                    format!(
                        "run `{}` to replace the token stored since",
                        phrase::command(ctx.args())
                    ),
                ));
            }
            // The token is stored for the origin this invocation targets, and is sent only
            // there. The old token is not read beyond that comparison, so a profile holding a
            // broken one can be repaired. Bound to `target`, the origin read before the
            // question, not a second read: a profile that changed meanwhile cannot bind the
            // token to an origin nobody confirmed.
            now.set_token(profile, token, &self.target.value)
        })?;
        Ok(json!({
            "profile": profile,
            "token_stored": true,
            "token_origin": origin,
            "config_path": self.path.display().to_string(),
        }))
    }
}

/// The prompt for the token, an unlabeled line on stderr as the confirmation prompt is
/// (tenet 15).
const TOKEN_PROMPT: &str =
    "paste the token from `https://console.hippius.com/dashboard/settings` (it is not shown): ";

/// The prompt for the access key, as [`TOKEN_PROMPT`] is.
const ACCESS_KEY_PROMPT: &str =
    "type or paste the 12 words of the access key (they are not shown): ";

/// The token a person pastes at the terminal, read without echo, for a login without
/// `--token-file`. Asked only when a person can answer (tenet 5); otherwise the run is refused,
/// as it was before the prompt existed, and the hint names `--token-file`.
fn asked_token(ctx: &Context<'_>) -> Result<Token> {
    if !ctx.can_prompt() {
        return Err(Error::usage(
            "`--token-file` is required when nobody at a terminal can be asked for the token",
            "pass `--token-file <path>`, or `--token-file -` to read the token from stdin",
        ));
    }
    let typed = hidden::read_line(|| output::note(TOKEN_PROMPT));
    // Enter was not echoed, so the next line starts here.
    output::note("\n");
    // The same command again, with the profile and the origin this run was given.
    let retry = format!(
        "run `{}` and paste the token at the prompt, or pass `--token-file <path>`",
        phrase::command(ctx.args())
    );
    token_from(typed, &retry)
}

/// The token in what [`hidden::read_line`] read; `retry` is the hint of a refusal.
fn token_from(typed: std::io::Result<Typed>, retry: &str) -> Result<Token> {
    match typed {
        Ok(Typed::Line(line)) => {
            let text = std::str::from_utf8(&line).map_err(|_| {
                Error::usage("the token typed at the prompt is not UTF-8 text", retry)
            })?;
            if text.trim().is_empty() {
                return Err(Error::usage("no token was typed at the prompt", retry));
            }
            Token::new(text, &Source::Argument).map_err(|error| error.with_hint(retry))
        }
        Ok(Typed::Ended) => Err(Error::usage("no token was typed at the prompt", retry)),
        Ok(Typed::Interrupted) => Err(Error::usage("the token prompt was interrupted", retry)),
        Ok(Typed::TooLong) => Err(Error::usage(
            "the line typed at the prompt is too long to be a token",
            retry,
        )),
        Err(error) => Err(Error::internal(format!(
            "cannot read the token at the terminal: {}",
            phrase::io_reason(&error)
        ))),
    }
}

/// The access key a person types at the terminal, read without echo, for `--access-key`.
/// Asked only when a person can answer (tenet 5); otherwise the hint names
/// `--access-key-file`.
fn asked_phrase(ctx: &Context<'_>) -> Result<Zeroizing<String>> {
    if !ctx.can_prompt() {
        return Err(Error::usage(
            "`--access-key` asks a person at a terminal for the 12 words, and nobody at a \
             terminal can be asked",
            "pass `--access-key-file <path>`, or `--access-key-file -` to read the 12 words from \
             stdin",
        ));
    }
    let typed = hidden::read_line(|| output::note(ACCESS_KEY_PROMPT));
    output::note("\n");
    let retry = format!(
        "run `{}` and type the 12 words at the prompt, or pass `--access-key-file <path>`",
        phrase::command(ctx.args())
    );
    phrase_from(typed, &retry)
}

/// The access key in what [`hidden::read_line`] read, moved into a buffer that is cleared when
/// dropped; `retry` is the hint of a refusal. No refusal quotes what was typed.
fn phrase_from(typed: std::io::Result<Typed>, retry: &str) -> Result<Zeroizing<String>> {
    match typed {
        Ok(Typed::Line(mut line)) => {
            // Moved, not copied: the bytes stay in the buffer the read filled.
            let text = String::from_utf8(std::mem::take(&mut *line)).map_err(|error| {
                // The bytes the conversion hands back are cleared too.
                drop(Zeroizing::new(error.into_bytes()));
                Error::usage(
                    "the access key typed at the prompt is not UTF-8 text",
                    retry,
                )
            })?;
            let text = Zeroizing::new(text);
            if text.trim().is_empty() {
                return Err(Error::usage("no access key was typed at the prompt", retry));
            }
            Ok(text)
        }
        Ok(Typed::Ended) => Err(Error::usage("no access key was typed at the prompt", retry)),
        Ok(Typed::Interrupted) => Err(Error::usage("the access key prompt was interrupted", retry)),
        Ok(Typed::TooLong) => Err(Error::usage(
            "the line typed at the prompt is too long to be an access key",
            retry,
        )),
        Err(error) => Err(Error::internal(format!(
            "cannot read the access key at the terminal: {}",
            phrase::io_reason(&error)
        ))),
    }
}

/// `hipc auth login --access-key` or `--access-key-file <path>`: the challenge, both
/// signatures, and the token the API answers, stored as a token file's is.
///
/// Every error is passed through [`hide_phrase`] on its way out. None quotes the phrase, since
/// only the file's path and a word's position are named; the redaction keeps it so if a
/// message ever changes.
fn login_with_access_key(
    ctx: &Context<'_>,
    access_key_file: Option<&Path>,
    new_account: bool,
) -> Result<Printed> {
    let login = Login::new(ctx)?;
    // Whether a token is stored now: the login's own token is not known before it answers, so
    // any stored one may be replaced, and is asked about first. The same token answered again
    // replaces nothing.
    let replaces = login.config.has_token(&login.profile);
    // Both questions come before the 12 words are read, so a person who declines has typed
    // nothing, and one asked about another origin sees it before the prompt for the words.
    if !ctx.dry_run() {
        // The token the login answers is sent to this origin from now on.
        ctx.confirm_origin(
            &login.target,
            "log in with the access key at",
            "the token it answers is then stored for it and sent there and nowhere else",
        )?;
        if replaces {
            login.confirm_replace(ctx, "the token the access key's login answers")?;
        }
    }
    let flag = "--access-key-file";
    let (phrase, source) = match access_key_file {
        // stdin has no owner or mode to check.
        Some(path) if path == Path::new("-") => {
            let input = Input {
                path,
                flag,
                what: "access key phrase",
            };
            (input.read_cleared_stdin()?, input.source())
        }
        // A file is held to the config file's checks.
        Some(path) => (
            hippius::read_phrase_file(path, flag)?,
            Source::File {
                flag,
                path: path.to_path_buf(),
            },
        ),
        None => (asked_phrase(ctx)?, Source::Argument),
    };
    let key = AccessKey::from_phrase(&phrase, &source);
    key.and_then(|key| open_session(ctx, &login, replaces, new_account, &key))
        .map_err(|error| hide_phrase(error, &phrase))
}

/// `error` with the phrase, as given and with its words joined by single spaces, replaced by
/// `<redacted>`. Nothing shorter is replaced: a single word, or a line of a phrase written one
/// word to a line, also appears in the messages' own text, and replacing it there would show
/// which words the phrase holds.
fn hide_phrase(error: Error, phrase: &str) -> Error {
    let joined = Zeroizing::new(phrase.split_whitespace().collect::<Vec<_>>().join(" "));
    let error = match phrase.trim() {
        "" => error,
        whole => error.redacted(whole),
    };
    if joined.is_empty() {
        error
    } else {
        error.redacted(&joined)
    }
}

/// Asks for the challenge, signs it, checks the token the verify answers with a read, and
/// stores it, for `login`, whose questions were asked before the key was read. A new account
/// is refused unless `new_account` allows it.
fn open_session(
    ctx: &Context<'_>,
    login: &Login,
    replaces: bool,
    new_account: bool,
    key: &AccessKey,
) -> Result<Printed> {
    let client = ctx.signed_out_client(&login.target)?;
    let challenge = Request::new(Method::Post, "/auth/mnemonic/")
        .json(&json!({
            "address": key.address(),
            "substrate_address": key.substrate_address(),
        }))
        .about(format!(
            "ask for a login challenge for {}",
            phrase::quote(key.substrate_address())
        ));
    let verify = |signature: &str, substrate_signature: &str| {
        Request::new(Method::Post, "/auth/verify/")
            .json(&json!({
                "signature": signature,
                "substrate_signature": substrate_signature,
                "address": key.address(),
                "substrate_address": key.substrate_address(),
            }))
            .about(format!(
                "log in as {}",
                phrase::quote(key.substrate_address())
            ))
    };
    if ctx.dry_run() {
        let unsigned = verify(
            "<the Ethereum signature of the challenge message>",
            "<the sr25519 signature of the challenge message>",
        );
        let placeholder = Token::new("dry-run", &Source::Default)
            .expect("the placeholder is one word of visible ASCII");
        // A dry run of requests, not a local `action`: the first request at the top, all three
        // in `requests`, and beside them the profile and origin the token would be stored for.
        let stored = login.dry_run(&placeholder, replaces)?;
        let mut doc = ctx.dry_run_document(&client, &challenge)?;
        let checked = ctx.signed_in_client(&login.target, &placeholder)?;
        doc["requests"] = json!([
            ctx.describe(&client, &challenge)?,
            ctx.describe(&client, &unsigned)?,
            ctx.describe(&checked, &token_check())?,
        ]);
        for field in [
            "profile",
            "token_stored",
            "replaces_token",
            "token_origin",
            "config_path",
        ] {
            doc[field] = stored[field].clone();
        }
        return Ok(Printed::Document(doc));
    }
    let answer = ctx
        .send(&client, &challenge)
        .and_then(|response| response.document())
        .map_err(rate_limit_hint)?;
    let (Some(message), Some(nonce)) = (
        answer.get("message").and_then(Value::as_str),
        answer.get("challenge").and_then(Value::as_str),
    ) else {
        return Err(replace::unreadable("ask for a login challenge"));
    };
    let signed = key.sign_login(message, nonce)?;
    let answer = ctx
        .send(
            &client,
            &verify(&signed.signature, &signed.substrate_signature),
        )
        .and_then(|response| response.document())
        .map_err(rate_limit_hint)?;
    let session = Session::read(&answer, key)?;
    // One mistyped word is a phrase the API has not seen, and it opens a new, empty account
    // rather than failing; storing its token would point every later command at that account.
    // The server opened it before it answered, so the refusal says so, and stores nothing.
    if session.is_new && !new_account {
        return Err(new_account_refused(key));
    }
    // A token the API itself refuses is not stored: the profile keeps the one it had.
    let checked = ctx.signed_in_client(&login.target, &session.token)?;
    ctx.send(&checked, &token_check()).map_err(|error| {
        error.with_hint("run the login again; the token it answered was not stored")
    })?;
    let mut doc = login.store(ctx, &session.token)?;
    doc["is_new"] = session.is_new.into();
    doc["address"] = key.address().into();
    doc["substrate_address"] = key.substrate_address().into();
    for field in ["user_id", "username"] {
        doc[field] = answer.get(field).cloned().unwrap_or(Value::Null);
    }
    // `HIPPIUS_TOKEN` wins over the stored token, so later commands do not act as the
    // account just logged in to while it is set.
    if login.overrides.token.is_some() {
        ctx.warning(
            "`HIPPIUS_TOKEN` is set, so later commands send it, not the token just stored; unset \
             it to act as the access key's account",
        );
    }
    if session.is_new {
        ctx.note(&format!(
            "a new account was opened for {}: it has no email and no free Drive plan, and S3 \
             needs no subscription",
            phrase::quote(key.substrate_address())
        ));
    }
    Ok(Printed::Document(doc))
}

/// The refusal of a login whose verify answered `is_new` without `--new-account`. The server
/// opened the account before it answered, so `side_effects` is `partial`, though nothing is
/// stored.
fn new_account_refused(key: &AccessKey) -> Error {
    Error::usage(
        format!(
            "the access key's words are new to the API, so it opened a new, empty account for \
             {}; no token was stored",
            phrase::quote(key.substrate_address())
        ),
        "check the 12 words and log in again; pass `--new-account` only to keep a new account \
         on purpose",
    )
    .with_side_effects(SideEffects::Partial)
}

/// What a verify answers: the token, and whether the account is new.
struct Session {
    token: Token,
    is_new: bool,
}

impl Session {
    /// Reads the verify `answer` for `key`. An answer for another address is refused, and its
    /// token not stored: storing it would log the profile in to an account the phrase does
    /// not hold.
    fn read(answer: &Value, key: &AccessKey) -> Result<Self> {
        let action = "log in with the access key";
        let token = answer
            .get("token")
            .and_then(Value::as_str)
            .and_then(|token| Token::new(token, &Source::Default).ok())
            .ok_or_else(|| replace::unreadable(action))?;
        let is_new = answer
            .get("is_new")
            .and_then(Value::as_bool)
            .ok_or_else(|| replace::unreadable(action))?;
        // Swagger's answer names `substrate_address` and not `address`, so only the first is
        // required; an `address` that is sent must be the key's, in any case (EIP-55 is only
        // a checksum in the letter case).
        let substrate = answer
            .get("substrate_address")
            .and_then(Value::as_str)
            .ok_or_else(|| replace::unreadable(action))?;
        let other = if substrate == key.substrate_address() {
            match answer.get("address") {
                None | Some(Value::Null) => None,
                Some(Value::String(address)) if address.eq_ignore_ascii_case(key.address()) => None,
                Some(other) => Some(
                    other
                        .as_str()
                        .map_or_else(|| other.to_string(), str::to_owned),
                ),
            }
        } else {
            Some(substrate.to_owned())
        };
        if let Some(other) = other {
            return Err(Error::internal(format!(
                "the login answered for {}, not for the access key's {}, so its token was not \
                 stored",
                phrase::quote(other),
                phrase::quote(key.substrate_address())
            )));
        }
        Ok(Self { token, is_new })
    }
}

/// The personal read that checks a token before it is stored, the one `hipc auth status`
/// checks with.
fn token_check() -> Request {
    Request::get("/accounts/").about("check the token the login answered")
}

/// A 429 from a login route counts the client address, not the account (swagger): logins,
/// failed verifies, and accounts opened today. Its hint says to wait.
fn rate_limit_hint(error: Error) -> Error {
    if error.code() == Code::RateLimited {
        error.with_hint(
            "wait for the `Retry-After` delay before logging in again: the API limits logins and \
             new accounts from one client address",
        )
    } else {
        error
    }
}

/// `hipc auth status`: proves the token with `GET /api/accounts/`, then names the signed-in
/// user from `GET /api/users/me/`, a personal route that gets no account header.
///
/// The token check decides the exit code. A failed user read does not undo it: the
/// command still exits 0, with `user` null and `user_error` saying what failed, and text
/// mode warns on stderr.
pub(crate) fn status(ctx: &Context<'_>) -> Result<Printed> {
    let config = context::config()?;
    let overrides = ctx.overrides()?;
    let (settings, client) = ctx.authed(&config)?;
    let check = Request::get("/accounts/").about("check the token");
    let me = Request::get("/users/me/").about("read the signed-in user");
    if let Step::DryRun(mut doc) = ctx.call(&client, &check, Gate::Open)? {
        doc["requests"] = ctx.describe_each(&client, &[&check, &me])?;
        return Ok(Printed::Document(doc));
    }
    let mut doc = json!({
        "profile": settings.profile,
        "account": settings.account.as_str(),
        "token_preview": settings.token().and_then(Token::preview),
        "token_source": if overrides.token.is_some() { "environment" } else { "config" },
        "api_url": settings.api_url,
    });
    if let Value::Object(fields) = &mut doc {
        fields.extend(account::selected_scope(&overrides, &settings));
    }
    match ctx
        .send(&client, &me)
        .and_then(|response| user(&response.document()?))
    {
        Ok(user) => doc["user"] = user,
        Err(error) => {
            ctx.warning(&format!(
                "the token works, but the signed-in user is unknown: {}",
                error.message()
            ));
            doc["user"] = Value::Null;
            doc["user_error"] =
                json!({ "code": error.code().as_str(), "message": error.message() });
        }
    }
    Ok(Printed::Document(doc))
}

/// The user in an answer of `GET /api/users/me/`, as the server sent it. Swagger
/// (`users_me`) documents a page of `User` rows, while its description says the route
/// returns the caller's own record, so both are read: a `User` object, or a page that
/// holds exactly one. A `User` is known by its `username`, the one field swagger requires.
fn user(answer: &Value) -> Result<Value> {
    let is_user = |value: &Value| value.get("username").is_some_and(Value::is_string);
    if is_user(answer) {
        return Ok(answer.clone());
    }
    match answer
        .get("results")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
    {
        Some([only]) if is_user(only) => Ok(only.clone()),
        _ => Err(replace::unreadable("read the signed-in user")),
    }
}

/// `hipc auth logout`: removes the stored token. Nothing is revoked on the server.
pub(crate) fn logout(ctx: &Context<'_>) -> Result<Printed> {
    let path = context::config_path()?;
    let overrides = ctx.overrides()?;
    let config = ConfigFile::load(path.clone())?;
    let profile = config.profile_name(&overrides)?;
    if ctx.dry_run() {
        return Ok(Printed::Document(local_dry_run(
            "remove_token",
            json!({ "profile": profile, "token_stored": false }),
        )));
    }
    ctx.confirm(Question {
        action: &format!("remove the stored token from the profile `{profile}`"),
        consequence: None,
    })?;
    // The question was about the token stored when it was asked. Another `hipc` can store or
    // replace one while the prompt waits; that token was not confirmed, so it stays, and the
    // run refuses. The check and the change share the lock of one edit. With no stored token
    // there is nothing to change, and no file to create: the check is a fresh read.
    let changed = || {
        Error::usage(
            format!(
                "the stored token of the profile `{profile}` changed while the question \
                 waited, so it was left in place"
            ),
            format!(
                "run `{}` to remove the token stored since",
                phrase::command(ctx.args())
            ),
        )
    };
    if config.has_token(&profile) {
        ConfigFile::edit(path, |now| {
            // Another logout removed it meanwhile: the goal is met.
            if !now.has_token(&profile) {
                return Ok(());
            }
            if !now.same_token(&profile, &config) {
                return Err(changed());
            }
            now.clear_token(&profile);
            Ok(())
        })?;
    } else if ConfigFile::load(path)?.has_token(&profile) {
        return Err(changed());
    }
    Ok(Printed::Document(json!({
        "profile": profile,
        "token_stored": false,
    })))
}

#[cfg(test)]
mod tests {
    use hippius::Code;
    use zeroize::Zeroizing;

    use super::*;

    /// What the prompt cannot turn into a token is `usage` with the retry hint, and a terminal
    /// that cannot be read is `internal`.
    #[test]
    fn a_typed_line_that_is_not_a_token_is_refused() {
        let line = |bytes: &[u8]| Ok(Typed::Line(Zeroizing::new(bytes.to_vec())));
        let refused = token_from(line(&[0xff, b'a']), "retry").unwrap_err();
        assert_eq!(refused.code(), Code::Usage);
        assert_eq!(
            refused.message(),
            "the token typed at the prompt is not UTF-8 text"
        );
        assert_eq!(refused.hint(), Some("retry"));
        let failed = token_from(Err(std::io::Error::other("gone")), "retry").unwrap_err();
        assert_eq!(failed.code(), Code::Internal);
        assert!(
            failed
                .message()
                .starts_with("cannot read the token at the terminal: ")
        );
        assert!(token_from(line(b" t0ken "), "retry").is_ok());
    }

    /// A message that ever quoted the phrase, as given or with its words on one line, prints
    /// `<redacted>` in its place; a single word or line of it is never replaced, so the
    /// message's own words cannot show which are in the phrase.
    #[test]
    fn the_phrase_is_redacted_from_any_error_and_nothing_shorter() {
        let phrase = "  alpha beta gamma\ndelta epsilon zeta\n";
        let error = Error::usage(
            "bad `alpha beta gamma\ndelta epsilon zeta` and `alpha beta gamma delta epsilon zeta`",
            "fix `alpha beta gamma` and the beta",
        );
        let hidden = hide_phrase(error, phrase);
        assert_eq!(hidden.message(), "bad `<redacted>` and `<redacted>`");
        assert_eq!(hidden.hint(), Some("fix `alpha beta gamma` and the beta"));
        let one_per_line = "access\nkey\nword\n";
        let hidden = hide_phrase(
            Error::usage("the access key list and the word", "pass the access key"),
            one_per_line,
        );
        assert_eq!(hidden.message(), "the access key list and the word");
        assert_eq!(hidden.hint(), Some("pass the access key"));
        let untouched = hide_phrase(Error::usage("x", "y"), " \n");
        assert_eq!(untouched.message(), "x");
    }

    /// What the prompt cannot turn into a phrase is `usage` with the retry hint and quotes
    /// nothing typed, and a terminal that cannot be read is `internal`.
    #[test]
    fn a_typed_line_that_is_not_a_phrase_is_refused() {
        let line = |bytes: &[u8]| Ok(Typed::Line(Zeroizing::new(bytes.to_vec())));
        let refused = phrase_from(line(&[b'a', 0xff]), "retry").unwrap_err();
        assert_eq!(refused.code(), Code::Usage);
        assert_eq!(
            refused.message(),
            "the access key typed at the prompt is not UTF-8 text"
        );
        assert_eq!(refused.hint(), Some("retry"));
        for (typed, message) in [
            (Typed::Ended, "no access key was typed at the prompt"),
            (Typed::Interrupted, "the access key prompt was interrupted"),
        ] {
            assert_eq!(
                phrase_from(Ok(typed), "retry").unwrap_err().message(),
                message
            );
        }
        let too_long = phrase_from(Ok(Typed::TooLong), "retry").unwrap_err();
        assert_eq!(
            too_long.message(),
            "the line typed at the prompt is too long to be an access key"
        );
        let failed = phrase_from(Err(std::io::Error::other("gone")), "retry").unwrap_err();
        assert_eq!(failed.code(), Code::Internal);
        assert!(
            failed
                .message()
                .starts_with("cannot read the access key at the terminal: ")
        );
        assert_eq!(&*phrase_from(line(b" a b "), "retry").unwrap(), " a b ");
    }
}
