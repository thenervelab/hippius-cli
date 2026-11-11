//! The ids a command takes as an argument, each a type its parser returns, and the argument
//! that names the thing a command acts on, declared once per id.
//!
//! A text id is a newtype that only its parser builds, so a value a command holds is one the
//! rule accepted, and it goes into a route as it is (`Request::to` takes it as `Display`). A
//! numeric id is a `u64`, which already holds its proof. Each `*Target` struct is the positional
//! id with its help line and parser, flattened into every command that takes it, so the help
//! reads the same everywhere and `hipc schema` keeps the field's name as the argument's id.

use std::fmt;

use clap::Args;
use serde::Serialize;

use crate::cli::values::{Charset, Dash, is_uuid, is_word, number, word};

/// A VM id, which goes into the URL path, so only the characters an id has are accepted. The
/// swagger types it as a string; 1 to 64 letters, digits, `-`, and `_` is a guess wide enough
/// for the UUIDs and slugs seen, and narrow enough that no `/` or `?` can change the route. An
/// App Engine app is a VM, so its id is one too.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub(crate) struct VmId(String);

impl VmId {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        word(
            value,
            1..=64,
            Charset::Slug,
            Dash::Taken,
            "a VM id is 1 to 64 letters, digits, `-`, and `_`",
        )
        .map(Self)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VmId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A CDN zone id: `z` and 12 characters. It goes into the URL path. Any ASCII letter or digit
/// is accepted for the 12, because the API does not document the alphabet or its case; the
/// check exists so that no `/`, `?`, or `..` can change the route.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub(crate) struct ZoneId(String);

impl ZoneId {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        let valid = value.len() == 13
            && value.starts_with('z')
            && value.bytes().all(|b| b.is_ascii_alphanumeric());
        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err("a zone id is `z` and 12 letters or digits, such as `zab12cd34ef56`".to_owned())
        }
    }
}

impl fmt::Display for ZoneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An S3 bucket name, by S3's naming rule: 3 to 63 lowercase letters, digits, `.`, and `-`,
/// starting and ending with a letter or digit, with no `..`. It goes into the path.
#[derive(Debug, Clone)]
pub(crate) struct BucketName(String);

impl BucketName {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        let bytes = value.as_bytes();
        // A name that starts with a letter or digit starts with no `-`.
        let valid = is_word(value, 3..=63, Charset::LowerDotDash, Dash::Refused)
            && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            && !value.contains("..");
        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err(
                "a bucket name is 3 to 63 lowercase letters, digits, `.`, and `-`, starts and \
                 ends with a letter or a digit, and has no `..`, such as `my-bucket.2`"
                    .to_owned(),
            )
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BucketName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An S3 token id: a UUID, which goes into the path.
#[derive(Debug, Clone)]
pub(crate) struct TokenId(String);

impl TokenId {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        if is_uuid(value) {
            Ok(Self(value.to_owned()))
        } else {
            Err("a token id is a UUID, from `hipc s3 tokens list`".to_owned())
        }
    }
}

impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A Hub namespace name: 1 to 63 lowercase letters, digits, and `-`, not starting with `-`,
/// which a command line would read as a flag.
#[derive(Debug, Clone)]
pub(crate) struct Namespace(String);

impl Namespace {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        word(
            value,
            1..=63,
            Charset::LowerDash,
            Dash::Refused,
            "a namespace name is 1 to 63 lowercase letters, digits, and `-`, and does not start \
             with `-`, such as `my-team`",
        )
        .map(Self)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A project id, which goes into the path. `default` is refused: the Default project is the
/// account itself and has no id these routes take.
pub(crate) fn project_id(value: &str) -> Result<hippius::ProjectId, String> {
    match hippius::ProjectId::parse(value, &hippius::Source::Argument) {
        Ok(Some(id)) => Ok(id),
        _ => Err(
            "a project id is `proj_` and 20 lowercase hex characters, from `hipc account \
             projects list`; the Default project has none"
                .to_owned(),
        ),
    }
}

/// A billing account id, which goes into the path; `default` is refused as for
/// [`project_id`].
pub(crate) fn billing_account_id(value: &str) -> Result<hippius::BillingId, String> {
    match hippius::BillingId::parse(value, &hippius::Source::Argument) {
        Ok(Some(id)) => Ok(id),
        _ => Err(
            "a billing account id is `bill_` and 20 lowercase hex characters, from `hipc \
             account billing-accounts list`; the Default billing account has none"
                .to_owned(),
        ),
    }
}

/// A billing account that may be the Default one: what a costs read puts in its path, and
/// what an accepted project transfer is billed to.
#[derive(Debug, Clone)]
pub(crate) enum BillingAccount {
    /// The Default billing account, `default` in a path.
    Default,
    /// A billing account of its own.
    Id(hippius::BillingId),
}

impl BillingAccount {
    /// `default` or a billing account id.
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match hippius::BillingId::parse(value, &hippius::Source::Argument) {
            Ok(id) => Ok(id.map_or(Self::Default, Self::Id)),
            Err(_) => Err(
                "a billing account is `default`, or `bill_` and 20 lowercase hex characters, \
                 from `hipc account billing-accounts list`"
                    .to_owned(),
            ),
        }
    }

    /// The billing account's own id; the Default one has none.
    pub(crate) fn id(&self) -> Option<&hippius::BillingId> {
        match self {
            Self::Default => None,
            Self::Id(id) => Some(id),
        }
    }
}

impl fmt::Display for BillingAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str(hippius::DEFAULT),
            Self::Id(id) => f.write_str(id.as_str()),
        }
    }
}

/// An account another account offers a project to: an account id, never `self`, which names
/// no account the server can find.
pub(crate) fn account_id(value: &str) -> Result<hippius::AccountId, String> {
    match hippius::Selection::parse(value, &hippius::Source::Argument) {
        Ok(hippius::Selection::Member(id)) => Ok(id),
        _ => Err("an account id is `acct_` and 20 lowercase hex characters".to_owned()),
    }
}

/// The refusal of a cluster id that is not a whole number.
const CLUSTER_ID: &str =
    "a cluster id is a whole number, from `hipc compute kubernetes clusters list`";
/// The refusal of a database id that is not a whole number.
const DATABASE_ID: &str = "a database id is a whole number, from `hipc compute databases list`";

/// The VM a command acts on.
#[derive(Debug, Args)]
pub(crate) struct VmTarget {
    /// The vm_id from hipc compute vms list
    #[arg(value_parser = VmId::parse)]
    pub(crate) vm_id: VmId,
}

/// The App Engine app a command acts on: a VM, listed by the apps list.
#[derive(Debug, Args)]
pub(crate) struct AppTarget {
    /// The vm_id from hipc compute app-engine apps list
    #[arg(value_parser = VmId::parse)]
    pub(crate) vm_id: VmId,
}

/// The CDN zone a command acts on.
#[derive(Debug, Args)]
pub(crate) struct ZoneTarget {
    /// The zone id from hipc cdn zones list
    #[arg(value_parser = ZoneId::parse)]
    pub(crate) zone_id: ZoneId,
}

/// The Kubernetes cluster a command acts on.
#[derive(Debug, Args)]
pub(crate) struct ClusterTarget {
    /// The cluster id from hipc compute kubernetes clusters list
    #[arg(value_parser = number(CLUSTER_ID))]
    pub(crate) cluster_id: u64,
}

/// The database a command acts on.
#[derive(Debug, Args)]
pub(crate) struct DatabaseTarget {
    /// The database id from hipc compute databases list
    #[arg(value_parser = number(DATABASE_ID))]
    pub(crate) database_id: u64,
}

/// The project a command acts on, by its id: the Default project has none.
#[derive(Debug, Args)]
pub(crate) struct ProjectTarget {
    /// The project id, proj_ and 20 lowercase hex characters, from hipc account projects list
    #[arg(value_parser = project_id)]
    pub(crate) project_id: hippius::ProjectId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vm_id_is_a_slug_of_1_to_64_and_prints_as_given() {
        let id = VmId::parse("-vm_1A").unwrap();
        assert_eq!((id.as_str(), id.to_string().as_str()), ("-vm_1A", "-vm_1A"));
        assert!(VmId::parse(&"a".repeat(64)).is_ok());
        for bad in ["", "a/b", "a?b", "a.b", &"a".repeat(65)] {
            assert_eq!(
                VmId::parse(bad).err().as_deref(),
                Some("a VM id is 1 to 64 letters, digits, `-`, and `_`"),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_zone_id_is_z_and_12_letters_or_digits() {
        assert_eq!(
            ZoneId::parse("zab12cd34ef56").unwrap().to_string(),
            "zab12cd34ef56"
        );
        assert!(ZoneId::parse("zAB12CD34EF56").is_ok());
        for bad in [
            "",
            "zab12cd34ef5",
            "zab12cd34ef567",
            "aab12cd34ef56",
            "zab12cd34ef/6",
        ] {
            assert!(ZoneId::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_bucket_name_follows_the_s3_rule() {
        for good in ["abc", "my-bucket.2", &"a".repeat(63)] {
            assert_eq!(BucketName::parse(good).unwrap().as_str(), good);
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
            assert!(BucketName::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(BucketName::parse("abc").unwrap().to_string(), "abc");
    }

    #[test]
    fn a_token_id_is_a_uuid() {
        let id = "0f8fad5b-d9cb-469f-a165-70867728950e";
        assert_eq!(TokenId::parse(id).unwrap().to_string(), id);
        for bad in [
            "",
            "0f8fad5b-d9cb-469f-a165",
            "0f8fad5b-d9cb-469f-a165-70867728950g",
            "0f8fad5bd9cb469fa16570867728950e",
            "../../x",
            "0f8fad5b-d9cb-469f-a165-70867728950e-",
        ] {
            assert_eq!(
                TokenId::parse(bad).err().as_deref(),
                Some("a token id is a UUID, from `hipc s3 tokens list`"),
                "{bad}"
            );
        }
    }

    /// The namespace rule: 1 to 63 of `a-z`, `0-9`, and `-`, not starting with `-`.
    #[test]
    fn a_namespace_name_holds_one_rule() {
        for good in ["a", "my-team", "a-", "0", &"a".repeat(63)] {
            let name = Namespace::parse(good).unwrap();
            assert_eq!((name.as_str(), name.to_string().as_str()), (good, good));
        }
        for bad in [
            "",
            &"a".repeat(64),
            "-a",
            "..",
            "a.b",
            "A",
            "a_b",
            "a/b",
            "a b",
            "é",
        ] {
            assert!(Namespace::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_selector_ids_take_an_id_and_refuse_the_default() {
        let project = format!("proj_{}", "a".repeat(20));
        let billing = format!("bill_{}", "b".repeat(20));
        let account = format!("acct_{}", "c".repeat(20));
        assert_eq!(project_id(&project).unwrap().to_string(), project);
        assert!(project_id("default").is_err());
        assert_eq!(billing_account_id(&billing).unwrap().to_string(), billing);
        assert!(billing_account_id("default").is_err());
        assert_eq!(account_id(&account).unwrap().to_string(), account);
        assert!(account_id("self").is_err());
        assert!(account_id("acct_x").is_err());
    }

    #[test]
    fn a_billing_account_is_default_or_an_id() {
        let billing = format!("bill_{}", "b".repeat(20));
        let default = BillingAccount::parse("default").unwrap();
        assert_eq!(
            (default.to_string(), default.id()),
            ("default".to_owned(), None)
        );
        let own = BillingAccount::parse(&billing).unwrap();
        assert_eq!(own.to_string(), billing);
        assert_eq!(
            own.id().map(hippius::BillingId::as_str),
            Some(billing.as_str())
        );
        assert!(BillingAccount::parse("bill_x").is_err());
    }
}
