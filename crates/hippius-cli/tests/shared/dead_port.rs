//! A loopback origin on which every connection is refused, for a test that must reach no
//! server: never a fixed port such as 9, where something may listen. Shared by the e2e and
//! the scripts tests, which each include this file.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::OnceLock;

/// `http://127.0.0.1:<port>`, the same for the whole test process.
pub(crate) fn no_server() -> &'static str {
    static ORIGIN: OnceLock<String> = OnceLock::new();
    ORIGIN.get_or_init(|| format!("http://{}", closed_port()))
}

/// Linux refuses a connection to a TCP socket that is bound and not listening, so the port is
/// held that way for the life of the process: no other socket can take it. The socket is
/// close-on-exec, so no process a test starts keeps it, and it is never closed.
#[cfg(target_os = "linux")]
static HELD: OnceLock<std::os::fd::OwnedFd> = OnceLock::new();

#[cfg(target_os = "linux")]
fn closed_port() -> SocketAddrV4 {
    use rustix::net::{AddressFamily, SocketType};
    let socket = rustix::net::socket(AddressFamily::INET, SocketType::STREAM, None)
        .expect("open a TCP socket");
    rustix::io::fcntl_setfd(&socket, rustix::io::FdFlags::CLOEXEC)
        .expect("mark the socket close-on-exec");
    rustix::net::bind(&socket, &SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .expect("bind a loopback port");
    let bound = SocketAddrV4::try_from(rustix::net::getsockname(&socket).expect("the port"))
        .expect("an IPv4 address");
    assert!(HELD.set(socket).is_ok(), "the port is held once");
    bound
}

/// macOS drops a connection to a socket that is bound and not listening, so a client waits
/// for its timeout: the port is bound, released, and checked to refuse a connection. A test
/// server that later takes the same port answers an unexpected request with a 404, which
/// fails that test; nothing reaches a server outside the test.
#[cfg(not(target_os = "linux"))]
fn closed_port() -> SocketAddrV4 {
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::time::Duration;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind a loopback port");
    let SocketAddr::V4(bound) = listener.local_addr().expect("the port") else {
        unreachable!("bound on 127.0.0.1");
    };
    drop(listener);
    let refused = TcpStream::connect_timeout(&SocketAddr::V4(bound), Duration::from_secs(2))
        .is_err_and(|error| error.kind() == std::io::ErrorKind::ConnectionRefused);
    assert!(
        refused,
        "the released port {bound} does not refuse a connection"
    );
    bound
}
