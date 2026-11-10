//! Client library for the Hippius console API, used by the `hipc` command.
//!
//! The library owns what `TENETS.md` makes a contract: the error object and its codes, the
//! config file, the account-header rules, and the HTTP client. It never prints. The `hipc`
//! binary owns argv, the output mode, and every byte written to stdout or stderr.

mod account;
mod app_secrets;
mod aws;
mod client;
mod clock;
mod config;
mod digest;
mod dns;
mod error;
mod json;
mod kubernetes;
pub mod phrase;
mod project;
mod regular;
mod source;
mod wallet;

pub use crate::account::{PROVISIONING_ROUTES, RouteFlag, SECRET_ROUTES, Selection, same_route};
pub use crate::app_secrets::{AppSecrets, clear_strings, is_secret_name};
pub use crate::aws::{AccessKeyId, AwsFiles, AwsProfile, SecretKey, configure};
pub use crate::client::{
    Client, Method, Request, Response, follow, is_default_origin, without_query_values,
};
pub use crate::clock::Now;
pub use crate::config::{
    ConfigFile, HubMarker, Overrides, Settings, Token, config_path, on_lock_wait, take_split_link,
};
pub use crate::digest::sha256_hex;
pub use crate::dns::{DnsCredentials, DnsProvider};
pub use crate::error::{CHECK_FIRST_HINT, Code, Error, ISSUES_URL, Result, SideEffects};
pub use crate::json::{
    CursorPage, Page, cursor_page, envelope, envelope_beside, number_holds, page, page_beside,
    redact_value, without_field,
};
pub use crate::kubernetes::{ClusterKey, MatchedKey, rfc3339_seconds};
pub use crate::project::{BillingId, DEFAULT, ProjectId};
pub use crate::regular::{OpenRegularError, open_regular};
pub use crate::source::{Given, Source};
pub use crate::wallet::{
    AccessKey, INVISIBLE_JOINS, LoginSignatures, holds_checked_phrase, holds_phrase,
    new_idempotency_key, phrase_in_argv, read_hotkey_file, read_phrase_file,
};
