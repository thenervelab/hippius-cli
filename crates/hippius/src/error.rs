//! The error type, and the JSON error object it renders as.
//!
//! Every code name and exit code comes from the closed table in `TENETS.md`, tenet 4. A
//! name that is not in that table is a contract change, not a refactor.

use serde_json::{Value, json};

/// Declares [`Code`] and [`Code::ALL`] from one list of variants, so a code cannot exist
/// without being in `ALL`, and so in `hipc schema`'s `errors` array. Rust has no stable way
/// to list an enum's variants, and an exhaustive match proves only that each variant is
/// handled, not that it is listed.
macro_rules! codes {
    (
        $(#[$meta:meta])*
        pub enum Code {
            $($(#[$doc:meta])* $variant:ident,)*
        }
    ) => {
        $(#[$meta])*
        pub enum Code {
            $($(#[$doc])* $variant,)*
        }

        impl Code {
            /// Every code, in the order of the tenet 4 table and of `hipc schema`'s `errors`
            /// array, which is the order of the variants.
            pub const ALL: [Self; [$(Self::$variant),*].len()] = [$(Self::$variant),*];
        }
    };
}

codes! {
    /// What failed, as the `code` field of the JSON error object.
    ///
    /// Tenet 4 lists the full closed set. A variant joins this enum with the first code
    /// path that produces it, so every variant here can happen today.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Code {
        /// The invocation is wrong: a flag, an argument, a variable, or a local file it
        /// reads.
        Usage,
        /// The command changes state and `--yes` was not given.
        ConfirmationRequired,
        /// The command returns a secret and `--reveal` was not given.
        RevealRequired,
        /// Subscribing to Hub again would charge twice: a paid plan is active and
        /// `--replace` was not passed, or a subscribe sent in the last 10 minutes may not
        /// be listed yet and the list shows no paid plan that subscribe did not cancel.
        ReplaceRequired,
        /// A write that replaces a whole list would remove entries the list has now, and
        /// `--remove-others` was not passed.
        RemoveOthersRequired,
        /// A CDN zone or bucket ACL write may make a private bucket readable by anyone, or a
        /// Hub namespace publicity write may open the namespace to anonymous pulls, and
        /// `--public` was not passed.
        PublicRequired,
        /// A `hipc api` call would change which peers of the network can reach each
        /// other, and `--change-isolation` was not passed.
        ChangeIsolationRequired,
        /// 401, or no token is configured.
        Auth,
        /// 403 without an account code the contract names.
        Permission,
        /// 404.
        NotFound,
        /// 409, or 412; or a state a read gave that rules the write out before it is sent.
        Conflict,
        /// 402.
        PaymentRequired,
        /// 400, 422, or any other status the contract does not name, 1xx included.
        Validation,
        /// 429.
        RateLimited,
        /// The request, or a wait such as a VM launch, ran out of time.
        Timeout,
        /// The connection failed, or 425, 502, 503, or 504.
        Unavailable,
        /// Part of a multi-call command failed. stdout still carries what succeeded.
        PartialFailure,
        /// 500, 501, a 5xx the contract does not name, a redirect (3xx, which hipc never
        /// follows), a response `hipc` cannot read, or output `hipc` could not write.
        Internal,
        /// The caller is not a member of the selected account.
        NotAMember,
        /// The route cannot be used in another account.
        AccountScopeUnsupported,
        /// The route does not take a project, and one is selected.
        ProjectScopeUnsupported,
        /// The route does not take a billing account, and one is selected.
        BillingScopeUnsupported,
        /// Only the account owner can do this.
        OwnerOnly,
        /// The member's level is too low.
        InsufficientGrant,
        /// The member needs the Manage the team permission.
        TeamManagementRequired,
        /// The member needs the Billing permission.
        BillingRequired,
    }
}

impl Code {
    /// This code's row of the code table: its name, exit code, meaning, and the flag an
    /// exit-2 code names. The match has no catch-all arm, so a new variant does not compile
    /// until it has a row. `hipc schema` prints this table, and a test checks the tenet 4
    /// table in TENETS.md against it.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per code keeps the closed table in one place, as tenet 4 lists it"
    )]
    fn row(self) -> Row {
        let row = |name, exit, meaning| Row {
            name,
            exit,
            meaning,
            flag: None,
        };
        // An exit-2 code that names the flag which gets past it.
        let flagged = |name, flag, meaning| Row {
            flag: Some(flag),
            ..row(name, 2, meaning)
        };
        match self {
            Self::Usage => row(
                "usage",
                2,
                "the invocation is wrong: a flag, an argument, a variable, or a local file it \
                 reads",
            ),
            Self::ConfirmationRequired => flagged(
                "confirmation_required",
                "--yes",
                "the command needs a confirmation, and `--yes` was not passed",
            ),
            Self::RevealRequired => flagged(
                "reveal_required",
                "--reveal",
                "the call returns a secret, and `--reveal` was not passed",
            ),
            Self::ReplaceRequired => flagged(
                "replace_required",
                "--replace",
                "a paid Hub plan is active and `--replace` was not passed, or a Hub subscribe \
                 was sent in the last 10 minutes and no paid plan is listed that it did not \
                 cancel",
            ),
            Self::RemoveOthersRequired => flagged(
                "remove_others_required",
                "--remove-others",
                "the new list leaves out entries the list has now, and `--remove-others` was not \
                 passed",
            ),
            Self::PublicRequired => flagged(
                "public_required",
                "--public",
                "the write may publish a private bucket or open the Hub namespace to anyone, \
                 and `--public` was not passed",
            ),
            Self::ChangeIsolationRequired => flagged(
                "change_isolation_required",
                "--change-isolation",
                "the call changes which peers of the network can reach each other, and \
                 `--change-isolation` was not passed",
            ),
            Self::Auth => row(
                "auth",
                1,
                "401, or no token is configured for the API origin",
            ),
            Self::Permission => row("permission", 1, "403 with no account code"),
            Self::NotFound => row("not_found", 1, "404"),
            Self::Conflict => row(
                "conflict",
                1,
                "409, or 412: another client changed the resource first; or a state `hipc` read \
                 that rules the write out, sent nothing, with `http_status` null",
            ),
            Self::PaymentRequired => row("payment_required", 1, "402"),
            Self::Validation => row(
                "validation",
                1,
                "400, 422, or any other status this table does not name",
            ),
            Self::RateLimited => row("rate_limited", 1, "429"),
            Self::Timeout => row("timeout", 1, "the request or the wait ran out of time"),
            Self::Unavailable => row(
                "unavailable",
                1,
                "the connection failed, or 425, 502, 503, or 504",
            ),
            Self::PartialFailure => row(
                "partial_failure",
                1,
                "part of a multi-call command failed, and stdout holds what was done",
            ),
            Self::Internal => row(
                "internal",
                1,
                "500, 501, a 5xx not listed under `unavailable`, a redirect (never followed), \
                 an answer `hipc` cannot read, or output `hipc` could not write",
            ),
            Self::NotAMember => row(
                "not_a_member",
                1,
                "the caller is not a member of the selected account",
            ),
            // Exit 2: nothing was done, and the fix is a flag, `--account self`.
            Self::AccountScopeUnsupported => flagged(
                "account_scope_unsupported",
                "--account",
                "the route cannot be used in another account, so `--account` must select the \
                 token's own",
            ),
            // Exit 2 like the account code: nothing was done, and the fix is a flag value,
            // `--project default` or `--billing default`.
            Self::ProjectScopeUnsupported => flagged(
                "project_scope_unsupported",
                "--project",
                "the route does not take a project, so `--project` must select the Default one",
            ),
            Self::BillingScopeUnsupported => flagged(
                "billing_scope_unsupported",
                "--billing",
                "the route does not take a billing account, so `--billing` must select the \
                 Default one",
            ),
            Self::OwnerOnly => row("owner_only", 1, "only the account owner can do this"),
            Self::InsufficientGrant => row(
                "insufficient_grant",
                1,
                "the member's grant on the product is too low",
            ),
            Self::TeamManagementRequired => row(
                "team_management_required",
                1,
                "the member needs the Manage the team permission",
            ),
            Self::BillingRequired => row(
                "billing_required",
                1,
                "the member needs the Billing permission",
            ),
        }
    }

    /// The `snake_case` name used in the JSON error object.
    pub fn as_str(self) -> &'static str {
        self.row().name
    }

    /// The process exit code: 2 when the caller must change the invocation, else 1.
    pub fn exit_code(self) -> u8 {
        self.row().exit
    }

    /// What the code means, as `hipc schema` lists it.
    pub fn meaning(self) -> &'static str {
        self.row().meaning
    }

    /// The flag an exit-2 code other than `usage` names: passing it gets past the refusal.
    pub fn flag(self) -> Option<&'static str> {
        self.row().flag
    }

    /// The account and grant codes a 403 body may carry, which pass through unchanged.
    pub(crate) fn from_account_refusal(code: &str) -> Option<Self> {
        match code {
            "not_a_member" => Some(Self::NotAMember),
            "account_scope_unsupported" => Some(Self::AccountScopeUnsupported),
            "project_scope_unsupported" => Some(Self::ProjectScopeUnsupported),
            "billing_scope_unsupported" => Some(Self::BillingScopeUnsupported),
            "owner_only" => Some(Self::OwnerOnly),
            "insufficient_grant" => Some(Self::InsufficientGrant),
            "team_management_required" => Some(Self::TeamManagementRequired),
            "billing_required" => Some(Self::BillingRequired),
            _ => None,
        }
    }
}

/// One row of the code table ([`Code::row`]).
#[derive(Debug)]
struct Row {
    name: &'static str,
    exit: u8,
    meaning: &'static str,
    flag: Option<&'static str>,
}

/// Whether the failed command may have changed anything on the server.
#[derive(Debug, Clone, Copy)]
pub enum SideEffects {
    /// Nothing changed: a read, a local refusal, or a write the server rejected.
    None,
    /// A write may have happened: the request was sent and the answer was a 5xx, a
    /// redirect, a timeout, or a broken connection.
    Partial,
}

impl SideEffects {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Partial => "partial",
        }
    }
}

/// The hint of a failure after which a write may have been done (`side_effects:
/// partial`): a retry could do it twice (tenet 4).
pub const CHECK_FIRST_HINT: &str =
    "check whether the change was made before running the command again";

/// The hint of a refusal about the selected account that `--account self` fixes.
pub(crate) const ACCOUNT_SELF_HINT: &str =
    "pass `--account self` to act in the token's own account";

/// Where a bug in `hipc` is reported.
pub const ISSUES_URL: &str = "https://github.com/thenervelab/hippius-cli/issues";

/// The hint of an `internal` error that has none of its own.
static INTERNAL_HINT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!("run the command again with `--debug`, and report what it prints at `{ISSUES_URL}`")
});

/// A failed command, rendered as the JSON error object.
///
/// `Display` is the message alone. Callers must not put a token or a secret in a message,
/// a hint, or the details: all three are printed on stderr.
///
/// The fields sit behind a `Box`, so the error side of a `Result` is one pointer.
#[derive(Debug, thiserror::Error)]
#[error("{}", .0.message)]
pub struct Error(Box<Inner>);

#[derive(Debug)]
struct Inner {
    code: Code,
    message: String,
    hint: Option<String>,
    request_id: Option<String>,
    http_status: Option<u16>,
    retry: Retry,
    side_effects: SideEffects,
    /// The parsed body of a non-2xx answer, or the failed parts of a multi-call command.
    details: Option<Value>,
    /// What a multi-call command did before it failed, printed on stdout (tenet 3).
    stdout: Option<Value>,
}

/// Whether the caller may retry, and after how long. `hipc` itself never retries
/// (tenet 4): a retry inside the CLI could charge twice.
#[derive(Debug)]
enum Retry {
    No,
    /// Safe to retry, after the server's `Retry-After` in milliseconds when it sent one.
    Yes {
        after_ms: Option<u64>,
    },
}

impl Error {
    /// A failure with no HTTP status, not retryable, with no side effects.
    ///
    /// # Panics
    ///
    /// For an exit-2 code: every exit-2 failure has a hint (tenet 4), so those are built
    /// with [`Error::usage`], [`Error::confirmation_required`], [`Error::reveal_required`],
    /// [`Error::replace_required`], [`Error::remove_others_required`],
    /// [`Error::public_required`], [`Error::account_scope_unsupported`],
    /// `Error::project_scope_unsupported`, or `Error::billing_scope_unsupported`, which take
    /// it.
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        assert_eq!(
            code.exit_code(),
            1,
            "an exit-2 error is built by the constructor that takes its hint"
        );
        Self::without_hint(code, message)
    }

    fn without_hint(code: Code, message: impl Into<String>) -> Self {
        Self(Box::new(Inner {
            code,
            message: message.into(),
            hint: None,
            request_id: None,
            http_status: None,
            retry: Retry::No,
            side_effects: SideEffects::None,
            details: None,
            stdout: None,
        }))
    }

    /// The invocation is wrong (`usage`, exit 2). `hint` is the one change that fixes it:
    /// an imperative that names the flag, variable, file, or command (tenet 15).
    pub fn usage(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::without_hint(Code::Usage, message).with_hint(hint)
    }

    /// The command needs a confirmation, and none was given (`confirmation_required`, exit
    /// 2). `action` is what it would have done, as the prompt names it, such as "delete
    /// the VM `vm_1`".
    pub fn confirmation_required(action: &str) -> Self {
        let flag = flag_of(Code::ConfirmationRequired);
        Self::without_hint(
            Code::ConfirmationRequired,
            format!("this would {action}, and `{flag}` was not passed"),
        )
        .with_hint(format!("pass `{flag}` to confirm"))
    }

    /// A person was asked to confirm `action` and did not answer yes
    /// (`confirmation_required`, exit 2). It reads differently from
    /// [`Error::confirmation_required`], where nobody could be asked.
    pub fn confirmation_declined(action: &str) -> Self {
        let flag = flag_of(Code::ConfirmationRequired);
        Self::without_hint(
            Code::ConfirmationRequired,
            format!("the prompt to {action} was not answered yes"),
        )
        .with_hint(format!("pass `{flag}` to confirm"))
    }

    /// The call returns a secret, and `--reveal` was not passed (`reveal_required`,
    /// exit 2). `argv` is the command line as given, which the hint repeats with the flag.
    pub fn reveal_required(argv: &[String]) -> Self {
        Self::without_hint(
            Code::RevealRequired,
            "the answer holds a secret, so the command needs `--reveal` to print it",
        )
        .with_hint(rerun_hint(flag_of(Code::RevealRequired), argv))
    }

    /// Subscribing to Hub again would charge twice (`replace_required`, exit 2). `argv` is
    /// the command line as given, which the hint repeats with `--replace`; a refusal the flag
    /// cannot get past (the double-subscribe window) replaces that hint with
    /// [`Error::with_hint`].
    pub fn replace_required(message: impl Into<String>, argv: &[String]) -> Self {
        let flag = flag_of(Code::ReplaceRequired);
        Self::without_hint(Code::ReplaceRequired, message).with_hint(rerun_hint(flag, argv))
    }

    /// A write that replaces a whole list would remove the entries `removed`, each a literal
    /// in backticks, and `--remove-others` was not passed (`remove_others_required`, exit 2).
    /// `argv` is the command line as given, which the hint repeats with the flag, followed by
    /// the entries that flag removes.
    pub fn remove_others_required(
        message: impl Into<String>,
        argv: &[String],
        removed: &[String],
    ) -> Self {
        let hint = format!(
            "{} to remove {}",
            rerun_hint(flag_of(Code::RemoveOthersRequired), argv),
            crate::phrase::join(removed, "and")
        );
        Self::without_hint(Code::RemoveOthersRequired, message).with_hint(hint)
    }

    /// A write would replace a whole list or document without reading it, and `--remove-others`
    /// was not passed (`remove_others_required`, exit 2): `hipc api`'s form of the guard, and that
    /// of `hipc s3 buckets acl update`, which reads nothing either. Nothing was read, so unlike
    /// [`Error::remove_others_required`] the hint cannot name what the write removes; it repeats
    /// the command line with the flag.
    pub fn remove_others_unread(message: impl Into<String>, argv: &[String]) -> Self {
        let hint = format!(
            "{} to replace it whole",
            rerun_hint(flag_of(Code::RemoveOthersRequired), argv)
        );
        Self::without_hint(Code::RemoveOthersRequired, message).with_hint(hint)
    }

    /// A CDN zone or bucket ACL write may publish a private bucket, or a Hub namespace update
    /// would open the namespace to anyone, and `--public` was not passed
    /// (`public_required`, exit 2). `argv` is the command line as given, which the hint
    /// repeats with the flag.
    pub fn public_required(message: impl Into<String>, argv: &[String]) -> Self {
        Self::without_hint(Code::PublicRequired, message)
            .with_hint(rerun_hint(flag_of(Code::PublicRequired), argv))
    }

    /// The route cannot act in another account, and one is selected
    /// (`account_scope_unsupported`, exit 2): refused by `hipc` before anything is sent, or by
    /// the server's 403. The hint is the one flag value that fixes both.
    pub fn account_scope_unsupported(message: impl Into<String>) -> Self {
        Self::without_hint(Code::AccountScopeUnsupported, message).with_hint(ACCOUNT_SELF_HINT)
    }

    /// The route does not take a project, and one is selected (`project_scope_unsupported`,
    /// exit 2): refused by `hipc` before anything is sent, or by the server's 403. The hint is
    /// the one flag value that fixes both.
    pub(crate) fn project_scope_unsupported(message: impl Into<String>) -> Self {
        Self::without_hint(Code::ProjectScopeUnsupported, message)
            .with_hint("pass `--project default` to act in the Default project")
    }

    /// The route does not take a billing account, and one is selected
    /// (`billing_scope_unsupported`, exit 2): refused by `hipc` before anything is sent, or by
    /// the server's 403. The hint is the one flag value that fixes both.
    pub(crate) fn billing_scope_unsupported(message: impl Into<String>) -> Self {
        Self::without_hint(Code::BillingScopeUnsupported, message)
            .with_hint("pass `--billing default` to use the Default billing account")
    }

    /// A `hipc api` call would change which peers of the network can reach each other, and
    /// `--change-isolation` was not passed (`change_isolation_required`, exit 2). `argv` is
    /// the command line as given, which the hint repeats with the flag.
    pub fn change_isolation_required(argv: &[String]) -> Self {
        let flag = flag_of(Code::ChangeIsolationRequired);
        let message = format!(
            "this would change which peers of the network can reach each other, and `{flag}` \
             was not passed"
        );
        Self::without_hint(Code::ChangeIsolationRequired, message).with_hint(rerun_hint(flag, argv))
    }

    /// A failure inside `hipc` (`internal`, exit 1).
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(Code::Internal, message)
    }

    /// Part of a multi-call command failed (`partial_failure`, exit 1). `stdout` is the
    /// document of what succeeded, and `details` lists what failed.
    /// `side_effects` says whether the parts that ran changed anything: `None` for a set of
    /// reads, `Partial` when a write went through before a later step failed.
    pub fn partial_failure(
        message: impl Into<String>,
        stdout: Value,
        details: Value,
        side_effects: SideEffects,
    ) -> Self {
        let mut error = Self::new(Code::PartialFailure, message).with_side_effects(side_effects);
        error.0.stdout = Some(stdout);
        error.0.details = Some(details);
        error
    }

    /// Marks a failure that came after a write went through: `side_effects` becomes
    /// `partial`, and `done` is printed on stdout so what was created is not lost
    /// (`TENETS.md` tenet 3; the VM launch whose wait ended without `running`). It is never
    /// retryable, whatever the failed step was: a retry would repeat the write.
    #[must_use]
    pub fn after_write(mut self, done: Value) -> Self {
        self.0.side_effects = SideEffects::Partial;
        self.0.retry = Retry::No;
        self.0.stdout = Some(done);
        self
    }

    /// The error with `edit` applied to the document it prints on stdout, when it has one;
    /// `side_effects` and `retryable` stay as they are.
    #[must_use]
    pub fn with_stdout_edited(mut self, edit: impl FnOnce(&mut Value)) -> Self {
        if let Some(stdout) = self.0.stdout.as_mut() {
            edit(stdout);
        }
        self
    }

    /// The error with every occurrence of `secret`, a value the caller sent such as an
    /// invite token, replaced by `<redacted>` in its message, hint, details, stdout document,
    /// and request id, so a server that repeats it cannot get it printed (tenet 9).
    ///
    /// # Panics
    ///
    /// When `secret` is empty.
    #[must_use]
    pub fn redacted(mut self, secret: &str) -> Self {
        let clean = |text: String| crate::json::redact(text, secret);
        let inner = &mut *self.0;
        inner.message = clean(std::mem::take(&mut inner.message));
        inner.hint = inner.hint.take().map(clean);
        inner.details = inner
            .details
            .take()
            .map(|details| crate::json::redact_value(details, secret));
        inner.stdout = inner
            .stdout
            .take()
            .map(|done| crate::json::redact_value(done, secret));
        inner.request_id = inner.request_id.take().map(clean);
        self
    }

    /// This error with `message` in place of its own and no `details`: for a route whose
    /// answer may repeat a secret the caller sent in a form `hipc` cannot redact, so nothing the
    /// server wrote is printed. The code, the status, the hint, `retryable`, `side_effects`,
    /// and `request_id` stay.
    #[must_use]
    pub fn without_server_words(mut self, message: impl Into<String>) -> Self {
        self.0.message = message.into();
        self.0.details = None;
        self
    }

    /// This error with `details` passed through `keep`, which returns what of the server's
    /// body may be printed, or `None` for nothing: for a route whose error body may hold a
    /// value the caller sent, where only fields `hipc` can vouch for stay. Everything else is
    /// kept.
    #[must_use]
    pub fn with_details_kept(mut self, keep: impl FnOnce(Value) -> Option<Value>) -> Self {
        self.0.details = self.0.details.take().and_then(keep);
        self
    }

    /// This error with its `request_id`, a header the server chose, passed through `keep`, for
    /// the same routes as [`Error::with_details_kept`].
    #[must_use]
    pub fn with_request_id_kept(mut self, keep: impl FnOnce(String) -> Option<String>) -> Self {
        self.0.request_id = self.0.request_id.take().and_then(keep);
        self
    }

    /// This error with `message` in place of its own, everything else kept: for a command
    /// that can say more than the server's words, such as what a `409` body lists, while
    /// `details` still holds what the server sent.
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.0.message = message.into();
        self
    }

    /// Adds the `hint` field: what the caller should do next.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.0.hint = Some(hint.into());
        self
    }

    #[must_use]
    pub(crate) fn retryable(mut self, after_ms: Option<u64>) -> Self {
        self.0.retry = Retry::Yes { after_ms };
        self
    }

    /// The server's `Retry-After`, in milliseconds, of a retryable error.
    pub(crate) fn retry_after_ms(&self) -> Option<u64> {
        match self.0.retry {
            Retry::Yes { after_ms } => after_ms,
            Retry::No => None,
        }
    }

    /// Sets `details`: the server's error body, or the transport error text of a request
    /// whose answer broke.
    #[must_use]
    pub(crate) fn with_details(mut self, details: Value) -> Self {
        self.0.details = Some(details);
        self
    }

    /// What the command did to the server before it failed.
    pub fn side_effects(&self) -> SideEffects {
        self.0.side_effects
    }

    /// Whether the caller may retry: the `retryable` field of the JSON.
    pub fn is_retryable(&self) -> bool {
        matches!(self.0.retry, Retry::Yes { .. })
    }

    /// Sets `side_effects`: what the command did to the server before it failed.
    #[must_use]
    pub fn with_side_effects(mut self, side_effects: SideEffects) -> Self {
        self.0.side_effects = side_effects;
        self
    }

    /// Sets `http_status` and `request_id`, from the answer the failure came with.
    #[must_use]
    pub(crate) fn with_http(mut self, status: u16, request_id: Option<String>) -> Self {
        self.0.http_status = Some(status);
        self.0.request_id = request_id;
        self
    }

    /// The contract code.
    pub fn code(&self) -> Code {
        self.0.code
    }

    /// The hint, if any.
    ///
    /// An `internal` error without one of its own gets `INTERNAL_HINT`: only a bug or a
    /// changed API reaches it, so the fix is a report (tenet 15). After a write that may have
    /// been done it gets [`CHECK_FIRST_HINT`] instead, such as a launch answered 2xx with a
    /// body `hipc` cannot read: running it again would launch, and bill, a second one.
    pub fn hint(&self) -> Option<&str> {
        match (&self.0.hint, self.0.code) {
            (Some(hint), _) => Some(hint),
            (None, Code::Internal) if matches!(self.0.side_effects, SideEffects::Partial) => {
                Some(CHECK_FIRST_HINT)
            }
            (None, Code::Internal) => Some(INTERNAL_HINT.as_str()),
            (None, _) => None,
        }
    }

    /// The document stdout carries despite the failure, if any.
    pub fn stdout(&self) -> Option<&Value> {
        self.0.stdout.as_ref()
    }

    /// The message alone, as `Display` prints it.
    pub fn message(&self) -> &str {
        &self.0.message
    }

    /// The HTTP status of the answer that failed, if a request got one.
    pub fn http_status(&self) -> Option<u16> {
        self.0.http_status
    }

    /// The parsed body of the non-2xx answer, or the failed parts of a multi-call command:
    /// the `details` field of the JSON.
    pub fn details(&self) -> Option<&Value> {
        self.0.details.as_ref()
    }

    /// The JSON error object, `{"error": {...}}`, with every field tenet 4 lists, in its
    /// order, `details` last: `null` when there are none, as every other field that does
    /// not apply is, so an agent reads one shape.
    pub fn to_json(&self) -> Value {
        let (retryable, retry_after_ms) = match self.0.retry {
            Retry::No => (false, None),
            Retry::Yes { after_ms } => (true, after_ms),
        };
        let object = json!({
            "code": self.0.code.as_str(),
            "message": self.0.message,
            "request_id": self.0.request_id,
            "hint": self.hint(),
            "retryable": retryable,
            "side_effects": self.0.side_effects.as_str(),
            "http_status": self.0.http_status,
            "retry_after_ms": retry_after_ms,
            "details": self.0.details,
        });
        json!({ "error": object })
    }
}

/// A `Result` whose error is [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// The flag an exit-2 code other than `usage` names.
fn flag_of(code: Code) -> &'static str {
    code.flag()
        .expect("an exit-2 code other than usage names a flag")
}

/// The hint of an exit-2 refusal that names a flag of the command, not a global one: the
/// whole command line to run, with `flag` added (tenet 15). A command line with a query in
/// it (a `hipc api` path) is not repeated, because a quoted URL never shows query values
/// (tenet 9); the hint then names the flag alone.
fn rerun_hint(flag: &str, argv: &[String]) -> String {
    if argv.iter().any(|word| word.contains('?')) {
        format!("pass `{flag}` after the command")
    } else {
        format!("run `{}`", crate::phrase::command_line(argv, flag))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ALL` holds every variant by construction (the `codes!` macro); this checks the
    /// rows: no two codes share a name. The tenet 4 test below checks the order.
    #[test]
    fn every_code_has_its_own_name() {
        let mut names: Vec<&str> = Code::ALL.iter().map(|c| c.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Code::ALL.len(), "two codes share a name");
    }

    /// The tenet 4 table in TENETS.md is this table: every row, in order, with the same
    /// name, exit code, and meaning.
    #[test]
    fn the_tenet_4_table_is_the_code_table() {
        let tenets = include_str!("../../../TENETS.md");
        let rows: Vec<(String, u8, String)> = tenets
            .lines()
            .filter_map(|line| {
                let mut cells = line.strip_prefix("| `")?.split(" | ");
                let (name, exit, meaning) = (cells.next()?, cells.next()?, cells.next()?);
                Some((
                    name.strip_suffix('`')?.to_owned(),
                    exit.parse().ok()?,
                    meaning.strip_suffix(" |")?.to_owned(),
                ))
            })
            .collect();
        let table: Vec<(String, u8, String)> = Code::ALL
            .iter()
            .map(|c| (c.as_str().to_owned(), c.exit_code(), c.meaning().to_owned()))
            .collect();
        assert_eq!(rows, table);
    }

    /// Each exit-2 code but `usage` names exactly one flag (tenet 4), and its meaning says
    /// which; no exit-1 code names one.
    #[test]
    fn exit_2_codes_other_than_usage_name_their_flag() {
        for code in Code::ALL {
            match (code.exit_code(), code.flag()) {
                (2, Some(flag)) => assert!(code.meaning().contains(flag), "{}", code.as_str()),
                (2, None) => assert_eq!(code, Code::Usage),
                (_, flag) => assert_eq!(flag, None, "{}", code.as_str()),
            }
        }
    }

    #[test]
    fn account_refusals_pass_through_and_nothing_else_does() {
        let refusals = [
            Code::NotAMember,
            Code::AccountScopeUnsupported,
            Code::ProjectScopeUnsupported,
            Code::BillingScopeUnsupported,
            Code::OwnerOnly,
            Code::InsufficientGrant,
            Code::TeamManagementRequired,
            Code::BillingRequired,
        ];
        for code in &refusals {
            let name = code.as_str();
            assert_eq!(Code::from_account_refusal(name), Some(*code), "{name}");
        }
        for other in ["usage", "permission", "project_not_found", ""] {
            assert_eq!(Code::from_account_refusal(other), None, "{other}");
        }
    }

    #[test]
    fn local_error_has_every_contract_field_in_order() {
        let json = Error::usage("bad flag", "pass a good one").to_json();
        let fields: Vec<&str> = json["error"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            fields,
            [
                "code",
                "message",
                "request_id",
                "hint",
                "retryable",
                "side_effects",
                "http_status",
                "retry_after_ms",
                "details"
            ]
        );
        assert_eq!(
            json,
            json!({"error": {
                "code": "usage",
                "message": "bad flag",
                "request_id": null,
                "hint": "pass a good one",
                "retryable": false,
                "side_effects": "none",
                "http_status": null,
                "retry_after_ms": null,
                "details": null
            }})
        );
    }

    #[test]
    fn http_error_carries_status_retry_side_effects_and_details_last() {
        let error = Error::new(Code::Unavailable, "chain unreachable")
            .with_http(503, Some("req-1".into()))
            .with_details(json!({"error": "x"}))
            .retryable(Some(5000))
            .with_side_effects(SideEffects::Partial);
        let json = error.to_json();
        assert_eq!(
            json,
            json!({"error": {
                "code": "unavailable",
                "message": "chain unreachable",
                "request_id": "req-1",
                "hint": null,
                "retryable": true,
                "side_effects": "partial",
                "http_status": 503,
                "retry_after_ms": 5000,
                "details": {"error": "x"}
            }})
        );
        assert_eq!(
            json["error"]
                .as_object()
                .unwrap()
                .keys()
                .next_back()
                .unwrap(),
            "details"
        );
        assert_eq!(error.http_status(), Some(503));
    }

    /// A reworded error keeps every field but its message, so a command that says more than
    /// the server's words loses none of what the server sent.
    #[test]
    fn with_message_replaces_only_the_message() {
        let error = Error::new(Code::Conflict, "the API answered 409")
            .with_http(409, Some("req-2".into()))
            .with_details(json!({"code": "project_not_empty"}))
            .retryable(Some(250))
            .with_side_effects(SideEffects::Partial)
            .with_hint("archive it again");
        let mut expected = error.to_json();
        expected["error"]["message"] = json!("it still holds the VM `a`");
        let reworded = error.with_message("it still holds the VM `a`");
        assert_eq!(reworded.to_json(), expected);
        assert_eq!(reworded.code(), Code::Conflict);
        assert_eq!(reworded.http_status(), Some(409));
        assert!(reworded.is_retryable());
        assert!(matches!(reworded.side_effects(), SideEffects::Partial));
        assert_eq!(expected["error"]["request_id"], "req-2");
        assert_eq!(expected["error"]["retry_after_ms"], 250);
        assert_eq!(
            expected["error"]["details"],
            json!({"code": "project_not_empty"})
        );
    }

    #[test]
    fn a_partial_failure_carries_stdout_and_details() {
        let error = Error::partial_failure(
            "one leg failed",
            json!({"items": [1]}),
            json!([{"service": "s3"}]),
            SideEffects::None,
        );
        assert_eq!(error.code(), Code::PartialFailure);
        assert_eq!(error.to_json()["error"]["side_effects"], "none");
        let after_write = Error::partial_failure("x", json!({}), json!([]), SideEffects::Partial);
        assert_eq!(after_write.to_json()["error"]["side_effects"], "partial");
        assert_eq!(error.stdout(), Some(&json!({"items": [1]})));
        assert_eq!(
            error.to_json()["error"]["details"],
            json!([{"service": "s3"}])
        );
        assert_eq!(error.message(), "one leg failed");
        assert_eq!(Error::usage("x", "y").stdout(), None);
    }

    #[test]
    fn an_error_after_a_write_keeps_its_code_and_prints_what_was_done() {
        let error = Error::new(Code::Unavailable, "bad gateway")
            .retryable(Some(1000))
            .after_write(json!({"vm_id": "v"}));
        assert_eq!(error.code(), Code::Unavailable);
        assert_eq!(error.stdout(), Some(&json!({"vm_id": "v"})));
        assert_eq!(error.to_json()["error"]["retryable"], false);
        assert_eq!(error.to_json()["error"]["retry_after_ms"], Value::Null);
        assert!(!error.is_retryable());
        assert!(
            Error::new(Code::Unavailable, "x")
                .retryable(None)
                .is_retryable()
        );
        assert_eq!(error.to_json()["error"]["side_effects"], "partial");
    }

    #[test]
    fn a_redacted_error_holds_the_secret_nowhere() {
        let error = Error::partial_failure(
            "cannot accept `s3cr3t`",
            json!({"echo": "s3cr3t"}),
            json!({"error": "bad s3cr3t"}),
            SideEffects::None,
        )
        .with_hint("check s3cr3t")
        // A server can echo the secret in `X-Request-Id` too.
        .with_http(404, Some("req-s3cr3t".into()))
        .redacted("s3cr3t");
        assert_eq!(error.message(), "cannot accept `<redacted>`");
        assert_eq!(error.hint(), Some("check <redacted>"));
        assert_eq!(error.details(), Some(&json!({"error": "bad <redacted>"})));
        assert_eq!(error.stdout(), Some(&json!({"echo": "<redacted>"})));
        assert!(!error.to_json().to_string().contains("s3cr3t"));
        let plain = Error::new(Code::NotFound, "x").redacted("s3cr3t");
        assert_eq!(plain.hint(), None);
        assert_eq!(plain.details(), None);
    }

    #[test]
    #[should_panic(expected = "an empty secret cannot be redacted")]
    fn redacting_an_empty_secret_is_a_bug() {
        drop(Error::internal("x").redacted(""));
    }

    #[test]
    fn hint_display_and_code() {
        assert_eq!(Error::internal("x").retry_after_ms(), None);
        let error = Error::usage("bad flag", "run `hipc --help`");
        assert_eq!(error.to_string(), "bad flag");
        assert_eq!(error.hint(), Some("run `hipc --help`"));
        assert_eq!(error.code(), Code::Usage);
        // An internal error without a hint of its own points at `--debug` and the tracker.
        let internal = Error::internal("x");
        let hint = internal.hint().unwrap();
        assert!(
            hint.contains("`--debug`") && hint.contains(ISSUES_URL),
            "{hint}"
        );
        assert_eq!(internal.to_json()["error"]["hint"], hint);
        assert_eq!(Error::internal("x").with_hint("y").hint(), Some("y"));
        assert_eq!(Error::new(Code::NotFound, "x").hint(), None);
        assert_eq!(Error::internal("x").code(), Code::Internal);
        assert_eq!(Error::usage("x", "y").http_status(), None);
    }

    /// A list-replace refusal repeats the command with `--remove-others` and names what that
    /// flag removes; a `--public` refusal repeats it with `--public`, and a
    /// `--change-isolation` refusal with `--change-isolation`.
    #[test]
    fn the_write_guards_repeat_the_command_with_their_flag() {
        let argv = ["cdn", "rules", "update", "z1", "--rules-file", "r.json"].map(String::from);
        let removed = ["`a`".to_owned(), "`b`".to_owned()];
        let error =
            Error::remove_others_required("this would remove 2 cache rules", &argv, &removed);
        assert_eq!(error.code(), Code::RemoveOthersRequired);
        assert_eq!(error.code().exit_code(), 2);
        assert_eq!(error.message(), "this would remove 2 cache rules");
        assert_eq!(
            error.hint(),
            Some(
                "run `hipc cdn rules update z1 --rules-file r.json --remove-others` to remove `a` \
                 and `b`"
            )
        );
        let error = Error::public_required("this may publish", &argv);
        assert_eq!(error.code(), Code::PublicRequired);
        assert_eq!(
            error.hint(),
            Some("run `hipc cdn rules update z1 --rules-file r.json --public`")
        );
        let argv = ["api", "POST", "/network/isolation/", "--yes"].map(String::from);
        let error = Error::change_isolation_required(&argv);
        assert_eq!(error.code(), Code::ChangeIsolationRequired);
        assert_eq!(error.code().exit_code(), 2);
        assert_eq!(
            error.message(),
            "this would change which peers of the network can reach each other, and \
             `--change-isolation` was not passed"
        );
        assert_eq!(
            error.hint(),
            Some("run `hipc api POST /network/isolation/ --yes --change-isolation`")
        );
    }
}
