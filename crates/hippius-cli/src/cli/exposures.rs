//! The grammar of `hipc compute exposures`.

use clap::{Args, Subcommand, ValueEnum};

use crate::cli::{EXPOSURE_ID, VmTarget, examples, number};

#[derive(Debug, Subcommand)]
pub(crate) enum Exposures {
    /// A VM's exposures, each with its id, endpoint, mode, and edge_sees_traffic
    #[command(after_help = examples(&[
        "hipc compute exposures list vm-1a2b",
    ]))]
    List(VmTarget),
    /// Make one guest port of a VM reachable from the internet; needs --yes
    ///
    /// The VM is otherwise reachable only on the private overlay. --mode http, the server's
    /// default, terminates TLS at the edge, so the edge can read the traffic. sni is end to end:
    /// the edge forwards ciphertext it cannot open, and the guest holds its own certificate. raw
    /// forwards one TCP or UDP port as the client sent it, on a public port the server picks, so
    /// the edge sees what the client sent. Exposing the same port and protocol again returns the
    /// existing exposure. The server refuses some ports, such as 22: SSH goes through the web
    /// terminal.
    #[command(after_help = examples(&[
        "hipc compute exposures create vm-1a2b --port 8080 --yes",
        "hipc compute exposures create vm-1a2b --port 443 --mode sni --yes",
    ]))]
    Create(ExposureCreate),
    /// Take an exposure down; needs --yes
    ///
    /// Its hostname cannot be used again.
    #[command(after_help = examples(&[
        "hipc compute exposures delete vm-1a2b 2 --yes",
    ]))]
    Delete {
        #[command(flatten)]
        vm: VmTarget,
        /// The exposure id from hipc compute exposures list
        #[arg(value_parser = number(EXPOSURE_ID))]
        exposure_id: u64,
    },
}

#[derive(Debug, Args)]
pub(crate) struct ExposureCreate {
    #[command(flatten)]
    pub(crate) vm: VmTarget,
    /// The guest port, 1 to 65535
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
    pub(crate) port: u16,
    /// How the edge forwards: http lets the edge read the traffic, sni is end to end, and raw
    /// forwards the bytes as sent
    ///
    /// Sent only when passed; the server defaults to http.
    #[arg(long, value_enum)]
    pub(crate) mode: Option<ExposeMode>,
    /// The protocol, tcp or udp; udp needs --mode raw
    ///
    /// Sent only when passed; the server defaults to tcp.
    #[arg(long, value_enum)]
    pub(crate) protocol: Option<ExposeProtocol>,
}

/// The `mode` values `POST .../expose/` accepts (swagger).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum ExposeMode {
    Http,
    Sni,
    Raw,
}

impl ExposeMode {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Sni => "sni",
            Self::Raw => "raw",
        }
    }
}

/// The `protocol` values `POST .../expose/` accepts (swagger).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum ExposeProtocol {
    Tcp,
    Udp,
}

impl ExposeProtocol {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }
}
