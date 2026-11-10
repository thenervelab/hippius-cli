//! `hipc auth`: store, check, and remove the console API token.

use std::path::{Path, PathBuf};

use hippius::routes;
use hippius::{ConfigFile, Error, Given, Overrides, Request, Result, Source, Token, phrase};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::commands::{Input, Printed, account, local_dry_run};
use crate::context::{self, Context, Gate, Question, Step};
use crate::hidden::{self, Typed};
use crate::output;

/// `hipc auth login`: `--token-file <path>`, or at a terminal the token typed at a prompt,
/// stores a token the caller holds.
pub(crate) fn login(ctx: &Context<'_>, token_file: Option<&Path>) -> Result<Printed> {
    let login = Login::new(ctx)?;
    // A token bound to any origin but the default one is sent there from now on. Asked
    // before the hidden prompt, the question says the token comes next: this answer is
    // shown as it is typed, so a token pasted here would be on the screen.
    let confirm_origin = |asked_next: bool| {
        let consequence = if asked_next {
            "the token is then sent there and nowhere else; answer here, and paste the token at \
             the hidden prompt that follows"
        } else {
            "the token is then sent there and nowhere else"
        };
        ctx.confirm_origin(&login.target, "store the token for", consequence)
    };
    let token = match token_file {
        Some(path) => {
            let input = Input {
                path,
                flag: "--token-file",
                what: "token",
            };
            Token::new(&Zeroizing::new(input.read_secret()?), &input.source())?
        }
        // A dry run sends nothing and needs no token (tenet 5), so nobody is asked for one: it
        // says whether a stored token would be replaced by whether one is stored at all.
        None if ctx.dry_run() => {
            let placeholder = placeholder_token();
            let replaces = login.config.has_token(&login.profile);
            return Ok(Printed::Document(local_dry_run(
                "store_token",
                login.dry_run(&placeholder, replaces)?,
            )));
        }
        // The origin is asked about before the prompt, so a person sees where the token goes
        // before pasting it, and one who declines has pasted nothing.
        None => asked_token(ctx, || confirm_origin(true))?,
    };
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
    if token_file.is_some() {
        confirm_origin(false)?;
    }
    if replaces {
        login.confirm_replace(ctx, "a different one")?;
    }
    let doc = login.store(ctx, &token)?;
    // `HIPPIUS_TOKEN` wins over the stored token, so later commands do not act as the token
    // just stored while it is set. `hipc auth status` checks the variable then, not the token
    // just stored, so no command checks it, and there is no note.
    if login.overrides.token.is_some() {
        ctx.warning(
            "`HIPPIUS_TOKEN` is set, so later commands send it, not the token just stored; unset \
             it to act as the stored token's owner",
        );
    } else if let Some(command) = ctx.next_command("auth status") {
        ctx.note(&format!(
            "the token is stored, not checked; run `{command}` to check it"
        ));
    }
    Ok(Printed::Document(doc))
}

/// The token a dry run stores in its copy of the config file when it has none to store, to
/// give the origin as the real run would store it.
fn placeholder_token() -> Token {
    Token::new("dry-run", &Source::Default).expect("the placeholder is one word of visible ASCII")
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

/// The token a person pastes at the terminal, read without echo, for a login without
/// `--token-file`, once `ask_first`, the origin question, passes. Asked only when a person can
/// answer (tenet 5); otherwise the run is refused before any question, as it was before the
/// prompt existed, and the hint names `--token-file`.
fn asked_token(ctx: &Context<'_>, ask_first: impl FnOnce() -> Result<()>) -> Result<Token> {
    if !ctx.can_prompt() {
        return Err(Error::usage(
            "`--token-file` is required when nobody at a terminal can be asked for the token",
            "pass `--token-file <path>`, or `--token-file -` to read the token from stdin",
        ));
    }
    ask_first()?;
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
    let check = Request::to(&routes::account::ACCOUNTS, []).about("check the token");
    let me = Request::to(&routes::account::USER, []).about("read the signed-in user");
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
        _ => Err(crate::commands::unreadable("read the signed-in user")),
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
}
