//! The grammar of `hipc hub`: plans, the subscription, the registry namespace, and its
//! repositories and artifacts.

use clap::{Args, Subcommand, ValueEnum};

use crate::cli::{
    HubEvents, HubKeys, HubModels, SUBSCRIPTION_ID, SecretFile, examples, number, reference,
};

#[derive(Debug, Subcommand)]
pub(crate) enum Hub {
    /// The Hub plan catalog; needs no token
    #[command(after_help = examples(&[
        "hipc hub plans",
    ]))]
    Plans,
    /// Read, buy, or cancel Hub plans
    ///
    /// Hub has no plan change: cancel, then subscribe.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc hub subscription get",
        "hipc hub subscription subscribe --plan Pro --yes",
    ]))]
    Subscription(HubSubscription),
    /// The registry namespace: whether a name is free (availability), create, inspect, and make
    /// public or private
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc hub namespace availability my-team",
        "hipc hub namespace create my-team --reveal --yes",
    ]))]
    Namespace(HubNamespace),
    /// The namespace's storage, repository, and artifact counts, with a 7-day history
    #[command(after_help = examples(&["hipc hub usage", "hipc hub usage --repositories"]))]
    Usage {
        /// List each repository with its artifact count and size instead
        #[arg(long)]
        repositories: bool,
    },
    /// The namespace's scoped CI keys: list, create, rotate, and delete
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc hub keys list",
        "hipc hub keys create --name ci --role push --reveal --yes",
    ]))]
    Keys(HubKeys),
    /// The namespace's repositories: list them, read one, and delete one
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc hub repositories list",
        "hipc hub repositories delete my-team/app --yes",
    ]))]
    Repositories(HubRepositories),
    /// List, inspect, and delete the artifacts in a repository: images and models, by tag or
    /// digest
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc hub artifacts list my-team/app",
        "hipc hub artifacts delete my-team/app v1 --yes",
    ]))]
    Artifacts(HubArtifacts),
    /// The namespace's audit events
    #[command(subcommand)]
    #[command(after_help = examples(&["hipc hub events list"]))]
    Events(HubEvents),
    /// The model index: the AI models and images pushed to public namespaces, and the
    /// account's own; needs no token
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc hub models list",
        "hipc hub models get my-team/llama --reference v1",
    ]))]
    Models(HubModels),
}

#[derive(Debug, Subcommand)]
pub(crate) enum HubRepositories {
    /// The namespace's repositories, with their artifact and pull counts
    #[command(after_help = examples(&["hipc hub repositories list"]))]
    List,
    /// One repository, by its name in the list
    #[command(after_help = examples(&["hipc hub repositories get my-team/app"]))]
    Get {
        /// The repository with its namespace, such as my-team/app, from hipc hub repositories list
        #[arg(value_parser = repository)]
        repository: String,
    },
    /// Delete a repository with every artifact and tag in it; needs --yes
    ///
    /// It cannot be undone. The lists can show it for a short while after the delete.
    #[command(after_help = examples(&[
        "hipc hub repositories delete my-team/app --yes",
    ]))]
    Delete {
        /// The repository with its namespace, such as my-team/app, from hipc hub repositories list
        #[arg(value_parser = repository)]
        repository: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum HubArtifacts {
    /// The artifacts in a repository, with their digests, tags, sizes, and push times
    #[command(after_help = examples(&["hipc hub artifacts list my-team/app"]))]
    List {
        /// The repository with its namespace, such as my-team/app, from hipc hub repositories list
        #[arg(value_parser = repository)]
        repository: String,
    },
    /// One artifact, by a tag or its digest
    #[command(after_help = examples(&[
        "hipc hub artifacts get my-team/app v1",
        "hipc hub artifacts get my-team/app sha256:0a1b2c",
    ]))]
    Get {
        /// The repository with its namespace, such as my-team/app, from hipc hub repositories list
        #[arg(value_parser = repository)]
        repository: String,
        /// A tag, such as v1, or the digest, such as sha256:0a1b2c
        #[arg(value_parser = reference)]
        reference: String,
    },
    /// Delete an artifact with every tag on it; needs --yes
    ///
    /// A tag names the artifact it points at, so deleting by one tag removes the artifact and
    /// its other tags too. It cannot be undone. The lists can show it for a short while after
    /// the delete.
    #[command(after_help = examples(&[
        "hipc hub artifacts delete my-team/app v1 --yes",
    ]))]
    Delete {
        /// The repository with its namespace, such as my-team/app, from hipc hub repositories list
        #[arg(value_parser = repository)]
        repository: String,
        /// A tag, such as v1, or the digest, such as sha256:0a1b2c
        #[arg(value_parser = reference)]
        reference: String,
    },
}

/// The longest repository name, in bytes: Docker's reference grammar caps a repository name
/// at 255 characters (`RepositoryNameTotalLengthMax`, `reference.go` of
/// `github.com/distribution/reference` at `0965666`).
const MAX_REPOSITORY_BYTES: usize = 255;

/// A Hub repository name as `hipc hub repositories list` prints it: the namespace, `/`, and
/// the repository, which may hold more `/`-separated parts. It goes into the URL path with
/// each `/` kept as a separator, as the console sends it.
///
/// Each part is ASCII letters, digits, `.`, `_`, and `-`, and is not `.` or `..`, so no part
/// can change the route. That is wider than the OCI name grammar, which has no uppercase;
/// the server judges the rest. A part named `artifacts` is refused: such a name would put
/// `/artifacts/` in a path that already means the artifact routes, and swagger does not say
/// which route the server would pick.
fn repository(value: &str) -> Result<String, String> {
    let parts: Vec<&str> = value.split('/').collect();
    let part_ok = |part: &&str| {
        !part.is_empty()
            && *part != "."
            && *part != ".."
            && *part != "artifacts"
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    if parts.len() >= 2 && value.len() <= MAX_REPOSITORY_BYTES && parts.iter().all(part_ok) {
        Ok(value.to_owned())
    } else {
        Err(format!(
            "a repository is named with its namespace, such as `my-team/app`, as `hipc hub \
             repositories list` prints it: at most {MAX_REPOSITORY_BYTES} bytes, in parts of \
             letters, digits, `.`, `_`, and `-` split by `/`, none of them `.`, `..`, or \
             `artifacts`"
        ))
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum HubSubscription {
    /// The Hub subscriptions, as the last chain sync saw them
    #[command(after_help = examples(&[
        "hipc hub subscription get",
    ]))]
    Get,
    /// Subscribe to a Hub plan, paid from the balance; spends money: needs --yes, and --replace
    /// while a paid plan is active
    ///
    /// Hub lets one account hold the same plan twice, so subscribing while a paid plan is active
    /// charges again. That is refused unless --replace is given, which cancels the active paid
    /// plans first. A new subscription appears in get only once its block is confirmed, a few
    /// minutes later; a subscribe from this profile in the last 10 minutes is refused the same way,
    /// even with --replace unless get lists a paid plan that subscribe did not cancel, but one
    /// made elsewhere in that window cannot be seen. The subscription is submitted, not yet active.
    #[command(after_help = examples(&[
        "hipc hub subscription subscribe --plan Pro --yes",
        "hipc hub subscription subscribe --plan Team --replace --yes",
    ]))]
    Subscribe(HubSubscribe),
    /// Cancel a Hub subscription by its id; needs --yes
    ///
    /// The namespace stays available for a 30-day grace period, and subscribing again within it
    /// keeps everything. When it ends, the namespace and everything in it are deleted for good
    /// (docs.hippius.com/use/console/hub).
    #[command(after_help = examples(&[
        "hipc hub subscription cancel 11 --yes",
    ]))]
    Cancel {
        /// The subscription_id from hipc hub subscription get
        #[arg(value_parser = number(SUBSCRIPTION_ID))]
        subscription_id: u64,
    },
}

#[derive(Debug, Args)]
pub(crate) struct HubSubscribe {
    /// The plan: its numeric id, or its name from hipc hub plans in any case
    #[arg(long)]
    pub(crate) plan: String,
    /// Cancel every active paid Hub plan first, so the new one is not charged on top
    #[arg(long)]
    pub(crate) replace: bool,
    // No `--pay-from`: Hub plans are paid from the balance only, and a flag whose one other
    // value is always refused would teach a caller to try it.
}

#[derive(Debug, Subcommand)]
pub(crate) enum HubNamespace {
    /// Whether a namespace name is free
    #[command(after_help = examples(&[
        "hipc hub namespace availability my-team",
    ]))]
    Availability {
        /// The namespace: 1 to 63 lowercase letters, digits, and dashes
        namespace: String,
    },
    // A new namespace is public by default in the docs (https://docs.hippius.com/registry/push).
    /// Create the registry namespace; needs --yes, and --reveal or --secret-file
    ///
    /// The docker robot secret is shown once, so --reveal, which prints it on stdout, or
    /// --secret-file, which writes it only to a new private file, is required before anything
    /// is sent.
    ///
    /// The Free plan needs a balance of at least $10 to create a namespace, a spam check the server
    /// enforces with 402.
    ///
    /// A new namespace is public: anyone can pull from it. To keep it private, run hipc hub
    /// namespace update --visibility private --yes before the first push.
    #[command(after_help = examples(&[
        "hipc hub namespace create my-team --reveal --yes",
        "hipc hub namespace create my-team --secret-file robot.secret --yes",
    ]))]
    Create {
        /// The namespace: 1 to 63 lowercase letters, digits, and dashes
        namespace: String,
        /// Print the one-time robot secret on stdout; without it or --secret-file nothing is sent
        #[arg(long)]
        reveal: bool,
        #[command(flatten)]
        secret_file: SecretFile,
    },
    /// The namespace's provisioning status
    #[command(after_help = examples(&[
        "hipc hub namespace status",
    ]))]
    Status,
    /// The active namespace; never shows a robot secret
    #[command(after_help = examples(&[
        "hipc hub namespace get",
    ]))]
    Get,
    /// Make the namespace public or private; needs --yes, and --public to make it public
    ///
    /// A public namespace lets anyone pull its images and models without logging in, and
    /// changes its storage allowance; a private one lets only logins with access pull.
    #[command(after_help = examples(&[
        "hipc hub namespace update --visibility public --public --yes",
        "hipc hub namespace update --visibility private --yes",
    ]))]
    Update {
        /// Who can pull: public is anyone, without logging in; private is logins with access
        #[arg(long)]
        visibility: Visibility,
        /// Allow the namespace to become public; required with --visibility public
        #[arg(long)]
        public: bool,
    },
    /// Read the docker robot login, or issue its new secret
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc hub namespace robot get",
        "hipc hub namespace robot rotate --reveal --yes",
    ]))]
    Robot(HubRobot),
}

/// Who can pull from the namespace (`PATCH /api/registry/me/publicity/ {public}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Visibility {
    /// Anyone, without logging in
    Public,
    /// Only logins with access to the namespace
    Private,
}

#[derive(Debug, Subcommand)]
pub(crate) enum HubRobot {
    /// The docker robot login and the docker login command for it; never shows a robot secret
    ///
    /// hipc prints the command and never runs it. The secret is shown only when it is issued:
    /// hipc hub namespace robot rotate --reveal --yes issues a new one and revokes the old.
    #[command(after_help = examples(&["hipc hub namespace robot get"]))]
    Get,
    /// Replace the docker robot secret; needs --yes, and --reveal or --secret-file
    ///
    /// The new secret is shown once, so --reveal, which prints it on stdout, or --secret-file,
    /// which writes it only to a new private file, is required before anything is sent. Every
    /// docker login that uses the old secret stops working. hipc prints the docker login command
    /// and never runs it.
    #[command(after_help = examples(&[
        "hipc hub namespace robot rotate --reveal --yes",
        "hipc hub namespace robot rotate --secret-file robot-new.secret --yes",
    ]))]
    Rotate {
        /// Print the new one-time robot secret on stdout; without it or --secret-file nothing is
        /// sent
        #[arg(long)]
        reveal: bool,
        #[command(flatten)]
        secret_file: SecretFile,
    },
}
