//! Client library for the Hippius console API, used by the `hipc` command.
//!
//! The library owns what `TENETS.md` makes a contract: the error object and its codes, the
//! config file, the account-header rules, and the HTTP client. It never prints. The `hipc`
//! binary owns argv, the output mode, and every byte written to stdout or stderr.

/// Ends a test that makes a file or directory unreadable or unwritable on purpose when the
/// suite runs as root, whom permission bits do not stop, instead of failing there.
#[cfg(test)]
macro_rules! skip_if_root {
    () => {
        if rustix::process::geteuid().is_root() {
            return;
        }
    };
}

mod account;
mod app_secrets;
mod aws;
mod client;
mod config;
mod digest;
mod dns;
mod encode;
mod error;
mod ids;
mod json;
mod kubernetes;
mod method;
pub mod phrase;
mod private_file;
mod private_object;
mod project;
mod redact;
mod regular;
pub mod routes;
mod secret;
mod selector;
mod source;
mod time;
mod url;
mod view;
mod wallet;

pub use crate::account::{api_guard, api_routes, same_route, secret_routes};
pub use crate::app_secrets::{AppSecrets, is_secret_name};
pub use crate::aws::{AccessKeyId, AwsFiles, AwsProfile, SecretKey, configure};
pub use crate::client::{
    Client, Request, Response, follow, is_default_origin, take_deprecation_warnings,
};
pub use crate::config::{
    ConfigFile, HubMarker, Overrides, Settings, Token, config_path, take_split_link,
    take_unknown_keys_warning,
};
pub use crate::digest::sha256_hex;
pub use crate::dns::{DnsCredentials, DnsProvider};
pub use crate::error::{CHECK_FIRST_HINT, Code, Error, Failure, ISSUES_URL, Result, SideEffects};
pub use crate::ids::new_idempotency_key;
pub use crate::json::{Next, Page, cursor_page, envelope, envelope_beside, page, page_beside};
pub use crate::kubernetes::{ClusterKey, MatchedKey, SignedOperation};
pub use crate::method::Method;
pub use crate::private_file::on_lock_wait;
pub use crate::redact::{
    holds_secret, insert_unique, redact_all, redact_name, redact_value, redact_value_all,
    redactions, without_field, without_fields,
};
pub use crate::regular::{OpenRegularError, open_regular};
pub use crate::routes::RouteFlag;
pub use crate::secret::SecretValue;
pub use crate::selector::{BillingId, DEFAULT, ProjectId, Selection};
pub use crate::source::{Given, Source};
pub use crate::time::{Now, date_of_unix_day, rfc3339_seconds};
pub use crate::url::without_query_values;
pub use crate::view::{Slot, TextOrNumber, view_of};
pub use crate::wallet::{
    Hotkey, INVISIBLE_JOINS, holds_checked_phrase, holds_phrase, phrase_in_argv, read_hotkey_file,
};
