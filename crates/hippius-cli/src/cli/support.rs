//! The grammar of `hipc support`.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use crate::cli::values::{Spaces, label};
use crate::cli::{
    ListArgs, MESSAGE_ID, SUPPORT_EXAMPLES, TICKET_ID, examples, examples_with_guide, number,
};

#[derive(Debug, Subcommand)]
pub(crate) enum Support {
    /// Open, list, read, reply to, and close support tickets, and read and attach files to
    /// their messages
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
    /// subject or message that holds an API token, HIPPIUS_TOKEN or one the config file stores,
    /// as it is, quoted, percent-encoded, or split by whitespace, or an access key whose
    /// checksum holds, is refused before anything is sent. The check catches an accident, such
    /// as a pasted config file, not a secret encoded on purpose.
    #[command(after_help = examples(&[
        "hipc support tickets create --subject Refund --body-file message.txt --yes",
        "hipc support tickets create --subject Down --priority urgent --body-file message.txt --yes",
        "hipc --dry-run support tickets create --subject Refund --body-file message.txt",
    ]))]
    Create(TicketCreate),
    /// Add a message to a ticket
    ///
    /// The message is read as for create. A reply is not confirmed: the ticket already exists,
    /// and a person reads the reply there. A message that holds an API token, HIPPIUS_TOKEN or
    /// one the config file stores, or an access key, is refused, as it is for create.
    #[command(after_help = examples(&[
        "hipc support tickets reply 42 --body-file message.txt",
    ]))]
    Reply {
        /// The ticket id, from hipc support tickets list
        #[arg(value_parser = number(TICKET_ID))]
        ticket_id: u64,
        #[command(flatten)]
        body: TicketBody,
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
    /// A ticket's messages, yours and the support team's, and the files attached to yours
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc support tickets messages list 42",
        "hipc support tickets messages attach 42 3 --file screenshot.png --yes",
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
    /// Attach a file to a message of a ticket, for the support team to read; needs --yes
    ///
    /// Sends the file as a multipart/form-data upload to the message, such as the one hipc
    /// support tickets reply answered with. The file is checked before anything is sent, the
    /// dry run included: it must hold 1 to 5000000 bytes (5 MB), and be a PNG, JPEG, GIF,
    /// WebP, or PDF file by its first bytes, or UTF-8 text, sent as text/plain; the file name
    /// is not looked at. The API documents no size or type limit, so these are hipc's own: the
    /// support chat's 5 MB for pictures, with PDF and text added. The server may still refuse
    /// a file they let through. A file that holds an API token, as it is or
    /// split by whitespace, or an access key, is refused, as a ticket's text is: the bytes are
    /// read as text, so a token in an image's metadata or an uncompressed PDF is found, but
    /// not one shown in a screenshot. The question names the file, its size, and the ticket.
    /// A dry run shows the file's name, type, and size in place of its bytes.
    #[command(after_help = examples(&[
        "hipc support tickets messages attach 42 3 --file screenshot.png --yes",
        "hipc --dry-run support tickets messages attach 42 3 --file vm.log",
    ]))]
    Attach(MessageAttach),
}

/// The file `hipc support tickets messages attach` sends, and the message it goes to.
#[derive(Debug, Args)]
pub(crate) struct MessageAttach {
    /// The ticket id, from hipc support tickets list
    #[arg(value_parser = number(TICKET_ID))]
    pub(crate) ticket_id: u64,
    /// The message id, from hipc support tickets messages list
    #[arg(value_parser = number(MESSAGE_ID))]
    pub(crate) message_id: u64,
    /// The file to attach, sent under its own name
    ///
    /// It is read only as a regular file, never through a symlink. stdin is refused: an
    /// attachment needs a file name, and the question may read the terminal.
    #[arg(long, value_name = "PATH", value_parser = attachment_file)]
    pub(crate) file: PathBuf,
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

/// `--file` of an attachment: any path but `-`, which every other file flag takes for stdin.
fn attachment_file(value: &str) -> Result<PathBuf, String> {
    if value == "-" {
        return Err("an attachment is read from a file, not stdin; pass its path".to_owned());
    }
    Ok(PathBuf::from(value))
}

/// A ticket subject. Swagger's `TicketCreate.subject` is 1 to 255 characters, and the
/// server trims it, so a blank one is refused here rather than by a 400 after the question.
/// A subject is a [`label`].
fn ticket_subject(value: &str) -> Result<String, String> {
    label(value, 1..=255, Spaces::Kept, "a subject")
}
