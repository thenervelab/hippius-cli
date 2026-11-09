//! Shared helpers: a private temporary directory, a static file server, and the lookup of
//! the system tools a test puts on the scripts' PATH.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A private directory under the system temp directory, removed on drop.
pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        // Created new with mode 0700, never reused, under a name with a random part: another
        // user who made the path first fails the test instead of owning its files.
        let random = std::hash::BuildHasher::hash_one(
            &std::collections::hash_map::RandomState::new(),
            NEXT.load(Ordering::Relaxed),
        );
        let path = std::env::temp_dir().join(format!(
            "hipc-scripts-{}-{}-{random:016x}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .expect("create a new private temporary directory");
        // The umask may have cleared bits of 0700, never added any.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .expect("make the temporary directory private");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best effort: a directory left under the temp dir does not fail a test.
        drop(std::fs::remove_dir_all(&self.path));
    }
}

/// A loopback origin that refuses every connection ([`crate::dead_port::no_server`]).
pub(crate) use crate::dead_port::no_server;

/// What the server answers for one path.
pub(crate) enum Answer {
    /// 200 with these bytes.
    File(Vec<u8>),
    /// 302 to this location, a path on the same server.
    Redirect(&'static str),
    /// 200 with a `Content-Length` of this many bytes, and no body: a file too large to
    /// download, refused by its header alone.
    Huge(u64),
}

/// Starts an HTTP/1.1 server on 127.0.0.1 that answers GET and HEAD for `routes` by path,
/// and 404 for every other path. Each connection carries one request. Returns the server's
/// base URL; the accept thread lives until the test process exits.
pub(crate) fn serve(routes: Vec<(String, Answer)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let port = listener.local_addr().expect("a bound address").port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            answer(&stream, &routes);
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn answer(stream: &TcpStream, routes: &[(String, Answer)]) {
    let mut reader = BufReader::new(stream);
    let mut first = String::new();
    drop(reader.read_line(&mut first));
    // The requests carry no body: read the headers up to the empty line that ends them.
    let mut line = String::new();
    while reader.read_line(&mut line).is_ok_and(|n| n > 0) && !line.trim_end().is_empty() {
        line.clear();
    }
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();

    let found = routes.iter().find(|(p, _)| p == path).map(|(_, a)| a);
    let (status, location, body, length): (&str, &str, &[u8], Option<u64>) = match found {
        Some(Answer::File(bytes)) => ("200 OK", "", bytes, None),
        Some(Answer::Redirect(to)) => ("302 Found", to, b"", None),
        Some(Answer::Huge(claimed)) => ("200 OK", "", b"", Some(*claimed)),
        None => ("404 Not Found", "", b"not found", None),
    };
    let length = length.unwrap_or(body.len() as u64);
    let mut head =
        format!("HTTP/1.1 {status}\r\nContent-Length: {length}\r\nConnection: close\r\n");
    if !location.is_empty() {
        head.push_str(&format!("Location: {location}\r\n"));
    }
    head.push_str("\r\n");
    let mut stream = stream;
    drop(stream.write_all(head.as_bytes()));
    if method != "HEAD" {
        drop(stream.write_all(body));
    }
}

/// The directories a system tool is looked up in, on macOS and on Linux.
const TOOL_DIRS: [&str; 6] = [
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
    "/usr/local/bin",
    "/opt/homebrew/bin",
];

/// The path of the system tool `name`, if it is installed in a standard directory.
pub(crate) fn find_tool(name: &str) -> Option<PathBuf> {
    TOOL_DIRS
        .iter()
        .map(|dir| Path::new(dir).join(name))
        .find(|path| path.is_file())
}

/// The path of the system tool `name`, which every system the tests run on has.
pub(crate) fn tool(name: &str) -> PathBuf {
    find_tool(name).unwrap_or_else(|| panic!("`{name}` is installed in a standard directory"))
}

/// The first of `names` that is not installed, for a test of a tool some systems lack (a
/// stock Mac has no `wget`, a minimal Linux no `shasum`), which then skips and says so.
pub(crate) fn missing_tool<'a>(names: &[&'a str]) -> Option<&'a str> {
    let missing = names.iter().copied().find(|name| find_tool(name).is_none());
    if let Some(name) = missing {
        // Shown with --nocapture; a skipped test otherwise reads as a pass.
        drop(writeln!(
            std::io::stderr(),
            "skipped: `{name}` is not installed"
        ));
    }
    missing
}

/// Writes an executable `sh` script named `name` into `dir`.
pub(crate) fn script(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write a fake tool");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make the fake tool executable");
}

/// The lower-case hex SHA-256 of the file at `path`, from `shasum`, which macOS has, or
/// `sha256sum`, which a minimal Linux has in its place.
pub(crate) fn sha256(path: &Path) -> String {
    let mut command = match find_tool("shasum") {
        Some(shasum) => {
            let mut command = Command::new(shasum);
            command.args(["-a", "256"]);
            command
        }
        None => Command::new(tool("sha256sum")),
    };
    let out = command.arg(path).output().expect("run the hash tool");
    assert!(out.status.success(), "the hash tool failed");
    String::from_utf8(out.stdout).expect("the hash tool prints ASCII")[..64].to_owned()
}
