//! The grammar of `hipc support`.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use crate::cli::values::{Spaces, is_uuid, label};
use crate::cli::{ListArgs, SUPPORT_EXAMPLES, TICKET_ID, examples, examples_with_guide, number};

#[derive(Debug, Subcommand)]
pub(crate) enum Support {
    /// Open, list, read, reply to, and close support tickets, and read their messages
    #[command(subcommand)]
    #[command(after_help = examples_with_guide(SUPPORT_EXAMPLES, "/use/console/support"))]
    Tickets(Tickets),
}

#[derive(Debug, Subcommand)]
pub(crate) enum Tickets {
    /// The signed-in user's tickets, newest activity first, one page at a time
    #[command(after_help = examples(&[
        "hipc support tickets list",
        "hipc support tickets list --status open",
        "hipc support tickets list --all",
    ]))]
    List(TicketList),
    /// One ticket, with its messages
    #[command(after_help = examples(&[
        "hipc support tickets get 42",
    ]))]
    Get {
        /// The ticket id, from hipc support tickets list
        #[arg(value_parser = number(TICKET_ID))]
        ticket_id: u64,
    },
    /// Open a ticket that a person on the support team reads; needs --yes
    ///
    /// The message is read from a file, or from stdin with --body-file -, never from argv. A
    /// subject, message, category, resource type, or resource id that holds an API token,
    /// HIPPIUS_TOKEN or one the config file stores, as it is, quoted, percent-encoded, or split
    /// by whitespace, or an access key whose checksum holds, is refused before anything is
    /// sent. The check catches an accident, such as a pasted config file, not a secret encoded
    /// on purpose.
    #[command(after_help = examples(&[
        "hipc support tickets create --subject Refund --body-file message.txt --yes",
        "hipc support tickets create --subject Down --priority urgent --body-file message.txt --yes",
        "hipc support tickets create --subject Down --category general --resource-type vm \
         --resource-id vm_1 --body-file message.txt --yes",
        "hipc --dry-run support tickets create --subject Refund --body-file message.txt",
    ]))]
    Create(TicketCreate),
    /// Add a message to a ticket
    ///
    /// The message is read as for create. A reply is not confirmed: the ticket already exists,
    /// and a person reads the reply there. A message that holds an API token, HIPPIUS_TOKEN or
    /// one the config file stores, or an access key, is refused, as it is for create. Each
    /// reply is sent with a client_msg_id, a new UUID unless --client-msg-id gives one; a dry
    /// run shows it in the body.
    #[command(after_help = examples(&[
        "hipc support tickets reply 42 --body-file message.txt",
        "hipc support tickets reply 42 --body-file message.txt \
         --client-msg-id 3f2c9a1e-7b4d-4e8a-9c01-5d6e7f8a9b0c",
    ]))]
    Reply {
        /// The ticket id, from hipc support tickets list
        #[arg(value_parser = number(TICKET_ID))]
        ticket_id: u64,
        #[command(flatten)]
        body: TicketBody,
        /// The client_msg_id to send, a UUID; without it each run sends a new one
        ///
        /// Pass the one a failed run or a dry run printed, to send the reply again under the
        /// same id. The API documents client_msg_id as a string of at most 64 characters and
        /// does not say what it does with an id it has seen, so a second run may still post
        /// the reply twice: check hipc support tickets messages list first.
        #[arg(long, value_name = "UUID", value_parser = client_msg_id)]
        client_msg_id: Option<String>,
    },
    /// Close a ticket; needs --yes
    ///
    /// Closing is the one change a customer makes to a ticket: its priority and assignee are
    /// the support team's. It reads the ticket first, so the question names its subject; a
    /// ticket already closed is a conflict, and nothing else is sent. A closed ticket stays
    /// readable.
    #[command(after_help = examples(&[
        "hipc support tickets update 42 --status closed --yes",
    ]))]
    Update {
        /// The ticket id, from hipc support tickets list
        #[arg(value_parser = number(TICKET_ID))]
        ticket_id: u64,
        /// The new status; closed is the one a customer may set
        #[arg(long)]
        status: TicketUpdateStatus,
    },
    /// A ticket's messages, yours and the support team's
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc support tickets messages list 42",
    ]))]
    Messages(TicketMessages),
}

#[derive(Debug, Subcommand)]
pub(crate) enum TicketMessages {
    /// A ticket's public messages, one page at a time
    ///
    /// The support team's internal notes are not shown. Add a message with
    /// hipc support tickets reply.
    #[command(after_help = examples(&[
        "hipc support tickets messages list 42",
        "hipc support tickets messages list 42 --all",
    ]))]
    List {
        /// The ticket id, from hipc support tickets list
        #[arg(value_parser = number(TICKET_ID))]
        ticket_id: u64,
        #[command(flatten)]
        list: ListArgs,
    },
}

/// The status a customer may give a ticket. Swagger's `TicketUpdate.status` lists four, but
/// the server restricts the staff fields to staff, and the console's only customer update
/// is `{"status": "closed"}`, so that is the one value offered.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum TicketUpdateStatus {
    Closed,
}

impl TicketUpdateStatus {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Closed => "closed",
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct TicketList {
    /// Only the tickets in this status
    #[arg(long)]
    pub(crate) status: Option<TicketStatus>,
    #[command(flatten)]
    pub(crate) list: ListArgs,
}

/// The ticket statuses of swagger's `support_tickets_list` filter.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum TicketStatus {
    Open,
    Pending,
    Resolved,
    Closed,
}

impl TicketStatus {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Pending => "pending",
            Self::Resolved => "resolved",
            Self::Closed => "closed",
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct TicketCreate {
    /// The subject line, 1 to 255 characters
    #[arg(long, value_parser = ticket_subject)]
    pub(crate) subject: String,
    /// How urgent it is; without it the server picks the default
    #[arg(long)]
    pub(crate) priority: Option<TicketPriority>,
    /// The ticket's category, one line of 1 to 50 characters
    ///
    /// Free text: the API lists no categories. The Hippius desktop app offers billing,
    /// storage, and general.
    #[arg(long, value_parser = ticket_category)]
    pub(crate) category: Option<String>,
    /// The kind of resource the ticket is about, one line of 1 to 50 characters
    ///
    /// Free text: the API lists no resource types.
    #[arg(long, value_parser = ticket_resource_type)]
    pub(crate) resource_type: Option<String>,
    /// The id of the resource the ticket is about, one line of 1 to 255 characters
    #[arg(long, value_parser = ticket_resource_id)]
    pub(crate) resource_id: Option<String>,
    #[command(flatten)]
    pub(crate) body: TicketBody,
}

/// `--body-file`, the message of a new ticket or of a reply.
#[derive(Debug, Args)]
pub(crate) struct TicketBody {
    /// The file holding the message, or - for stdin; it must not be blank
    ///
    /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
    /// refused, so pass - for stdin.
    #[arg(long, value_name = "PATH")]
    pub(crate) body_file: PathBuf,
}

/// The ticket priorities of swagger's `TicketCreate`.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum TicketPriority {
    Low,
    Normal,
    High,
    Urgent,
}

impl TicketPriority {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
            Self::Urgent => "urgent",
        }
    }
}

/// A ticket subject. Swagger's `TicketCreate.subject` is 1 to 255 characters, and the
/// server trims it, so a blank one is refused here rather than by a 400 after the question.
/// A subject is a [`label`].
fn ticket_subject(value: &str) -> Result<String, String> {
    label(value, 1..=255, Spaces::Kept, "a subject")
}

/// `--category`. Swagger's `TicketCreate.category` is a string of at most 50 characters with
/// no list of values, so any [`label`] that long is taken.
fn ticket_category(value: &str) -> Result<String, String> {
    label(value, 1..=50, Spaces::Kept, "a category")
}

/// `--resource-type`. Swagger's `TicketCreate.resource_type` is a string of at most 50
/// characters with no list of values, so any [`label`] that long is taken.
fn ticket_resource_type(value: &str) -> Result<String, String> {
    label(value, 1..=50, Spaces::Kept, "a resource type")
}

/// `--resource-id`. Swagger's `TicketCreate.resource_id` is a string of at most 255
/// characters, and a person reads it on the ticket, so it is a [`label`].
fn ticket_resource_id(value: &str) -> Result<String, String> {
    label(value, 1..=255, Spaces::Kept, "a resource id")
}

/// `--client-msg-id`: a UUID, as each reply makes one. Swagger allows any string of at most 64
/// characters; a UUID is 36, so the id a run printed always fits.
fn client_msg_id(value: &str) -> Result<String, String> {
    if is_uuid(value) {
        Ok(value.to_owned())
    } else {
        Err(
            "a client_msg_id is a UUID such as `3f2c9a1e-7b4d-4e8a-9c01-5d6e7f8a9b0c`, as a reply \
             prints it"
                .to_owned(),
        )
    }
}
