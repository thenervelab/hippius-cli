//! Shared helpers: a scripted local HTTP server that records every request, a private
//! temporary home, and a builder that runs the `hipc` binary against both.

use std::ffi::OsString;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use hippius::{Client, ConfigFile, Given, Method, Overrides, Request, Settings, Source, Token};
use serde_json::Value;

mod fixtures;
pub(crate) mod terminal;
mod watchdog;

pub(crate) use fixtures::{BILLING, OWN_ACCOUNT, PROJECT, VM, api, parsed, permission_bits_apply};
pub(crate) use watchdog::{finish, finished};

/// The token every test uses. Every run asserts it never appears in any output.
pub(crate) const TOKEN: &str = "tok_secret_0123456789abcdefghijklmnopqrstuv";

/// The seed phrases the tests use: a gamma hotkey's words, and the access key a support ticket
/// or a command line must not carry. No run prints one, or sends one in a request, or any run
/// of four of its words in order, whatever the outcome.
pub(crate) const SEED_PHRASES: [&str; 2] = [
    "bottom drive obey lake curtain smoke basket hold race lonely fit walk",
    "test test test test test test test test test test test junk",
];

/// Fails when `text`, what `place` holds, has four words of a [`SEED_PHRASES`] phrase in order.
fn assert_no_phrase(text: &str, place: &str) {
    for phrase in SEED_PHRASES {
        let words: Vec<&str> = phrase.split(' ').collect();
        for run in words.windows(4) {
            let run = run.join(" ");
            assert!(!text.contains(&run), "a seed phrase is in {place}: {text}");
        }
    }
}

/// A membership account id.
pub(crate) const MEMBER: &str = "acct_0123456789abcdef0123";

/// The accounts list the console answers, with the caller's own account first.
pub(crate) const ACCOUNTS: &str = r#"{"sharing_enabled":true,"accounts":[
    {"id":"acct_ffffffffffffffffffff","name":"me@example.com","is_owner":true},
    {"id":"acct_0123456789abcdef0123","name":"team@example.com","is_owner":false}
]}"#;

/// Every secret key of the cluster key files the tests write, [`CLUSTER_KEY`] and
/// [`OTHER_KEY`], in the base64 the files hold. Every run asserts none appears on stdout or
/// stderr, and every [`Server`] that none appears in a request it received.
/// `scripts/release-check.sh` greps its output for the same list.
pub(crate) const CLUSTER_SECRETS: [&str; 3] = [
    "CwwNDg8QERITFBUWFxgZGhscHR4fICEiIyQlJicoKSo=",
    "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA=",
    "ZWZnaGlqa2xtbm9wcXJzdHV2d3h5ent8fX5/gIGCg4Q=",
];

/// A Kubernetes cluster key file, as the console saves it, with the keys of the console's test
/// vectors: the Ed25519 seed is the bytes 11 to 42 and the X25519 secret the bytes 1 to 32. A
/// public test value, never a real key.
pub(crate) const CLUSTER_KEY: &str = r#"{
  "version": 1,
  "kind": "hippius-k8s-cluster-key",
  "cluster_name": "prod",
  "created_at": "2026-10-01T00:00:00.000Z",
  "sign_secret_key": "CwwNDg8QERITFBUWFxgZGhscHR4fICEiIyQlJicoKSo=",
  "sign_public_key": "VCgh6VKd0a0kO/3/XGQKZRlLSBaYemUN6GHXzp5BemM=",
  "box_secret_key": "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA=",
  "box_public_key": "B6N8vBQgk8i3VdwbEOhstCY3StFqqFPtC9/AsrhtHHw="
}
"#;

/// A key file that parses, but is not the cluster's: its signing seed is the bytes 101 to 132,
/// and its box key is [`CLUSTER_KEY`]'s.
pub(crate) const OTHER_KEY: &str = r#"{
  "version": 1,
  "kind": "hippius-k8s-cluster-key",
  "sign_secret_key": "ZWZnaGlqa2xtbm9wcXJzdHV2d3h5ent8fX5/gIGCg4Q=",
  "sign_public_key": "2inpWwLgD/oVZFd1+x0roiKhlDOV7qBrlOLAV7e+adA=",
  "box_secret_key": "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA=",
  "box_public_key": "B6N8vBQgk8i3VdwbEOhstCY3StFqqFPtC9/AsrhtHHw="
}"#;

/// The signed-in user, as `GET /api/users/me/` answers in swagger's `User` shape.
pub(crate) const ME: &str = r#"{"id":7,"username":"ana","email":"ana@example.com"}"#;

/// The hint of a Hub command run by [`Hipc::against`] `server` before the account has a Hub
/// project: the create, with the origin the run was given in `HIPPIUS_API_URL`.
pub(crate) fn hub_provision_hint(server: &Server) -> String {
    format!(
        "run `hipc --api-url {} hub namespace create <namespace> --reveal --yes` to create the \
         account's Hub namespace",
        server.url()
    )
}

/// The two reads of `hipc auth status`: the token check and the signed-in user.
pub(crate) fn status_routes() -> Vec<Route> {
    vec![
        route("GET", "/api/accounts/", Reply::json(200, ACCOUNTS)),
        route("GET", "/api/users/me/", Reply::json(200, ME)),
    ]
}

/// The `User-Agent` every request carries: the API answers a default client agent with 403,
/// error 1010 (tenet 8).
const USER_AGENT: &str = concat!("hipc/", env!("CARGO_PKG_VERSION"));

/// One request the server received.
#[derive(Debug, Clone)]
pub(crate) struct Recorded {
    pub(crate) method: String,
    /// The path and query, as sent.
    pub(crate) target: String,
    /// Header names in lower case, in the order sent, repeats kept.
    headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
    /// Whether a route matched, or the server made up its 404.
    routed: bool,
    /// When the server finished reading it.
    pub(crate) received: Instant,
}

impl Recorded {
    /// The value of the header `name` (lower case), which must not be repeated.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        let values = self.values(name);
        assert!(
            values.len() <= 1,
            "the header `{name}` is repeated on {} {}",
            self.method,
            self.target
        );
        values.first().copied()
    }

    /// Every value of the header `name` (lower case), in the order sent.
    fn values(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// The headers every request carries, whatever the run's settings: one `User-Agent`
    /// of this version, and at most one `Authorization`, in the `Token` scheme, and one
    /// each of `X-Hippius-Account`, `X-Hippius-Project`, and `X-Hippius-Billing`.
    fn assert_fixed_headers(&self) {
        let request = format!("{} {}", self.method, self.target);
        assert_eq!(self.header("user-agent"), Some(USER_AGENT), "{request}");
        if let Some(authorization) = self.header("authorization") {
            assert!(
                authorization.starts_with("Token "),
                "{request}: not the Token scheme"
            );
        }
        for name in [
            "x-hippius-account",
            "x-hippius-project",
            "x-hippius-billing",
        ] {
            assert!(
                self.values(name).len() <= 1,
                "{request}: `{name}` is repeated"
            );
        }
    }

    pub(crate) fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("the request body is JSON")
    }

    pub(crate) fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }
}

/// A scripted answer.
#[derive(Debug, Clone)]
pub(crate) struct Reply {
    status: u16,
    /// Bytes, which need not be UTF-8. `{base}` is replaced only in a UTF-8 body.
    body: Vec<u8>,
    headers: Vec<(String, String)>,
    /// Hold the connection open without answering until the client gives up.
    never: bool,
    /// Run on the server thread after the request is read and before the answer is sent,
    /// so a test can change the world between what `hipc` checked first and what it does
    /// with the answer.
    before: Option<Hook>,
    /// Written as it is instead of an answer, before the connection is held as [`Reply::never`]
    /// holds it.
    raw: Option<Vec<u8>>,
    /// Makes the answer from the request ([`Reply::from_fn`]).
    answer: Option<Answer>,
}

/// What [`Reply::from_fn`] answers with.
#[derive(Clone)]
pub(crate) struct Answer(Arc<dyn Fn(&Recorded) -> Reply + Send + Sync>);

impl std::fmt::Debug for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Answer")
    }
}

/// A step [`Reply::before`] runs.
#[derive(Clone)]
pub(crate) struct Hook(Arc<dyn Fn() + Send + Sync>);

impl std::fmt::Debug for Hook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Hook")
    }
}

impl Reply {
    pub(crate) fn json(status: u16, body: &str) -> Self {
        Self::bytes(status, "application/json", body.as_bytes())
    }

    /// A body of any bytes, with its `Content-Type`.
    pub(crate) fn bytes(status: u16, content_type: &str, body: &[u8]) -> Self {
        Self {
            status,
            body: body.to_vec(),
            headers: vec![("Content-Type".into(), content_type.into())],
            never: false,
            before: None,
            raw: None,
            answer: None,
        }
    }

    /// The answer `answer` makes from each request, for a test whose answer depends on the
    /// request: the page it asks for, or how many came before it. A reply `answer` returns
    /// is sent as it is, its own `from_fn` ignored.
    pub(crate) fn from_fn(answer: impl Fn(&Recorded) -> Self + Send + Sync + 'static) -> Self {
        Self {
            answer: Some(Answer(Arc::new(answer))),
            ..Self::json(200, "")
        }
    }

    /// `bytes` as they are, a status line and headers included, then nothing until the
    /// client gives up and closes: for an answer no well-formed reply gives, such as a body
    /// shorter than its `Content-Length`.
    pub(crate) fn raw(bytes: &[u8]) -> Self {
        Self {
            raw: Some(bytes.to_vec()),
            ..Self::json(200, "")
        }
    }

    pub(crate) fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// This answer, sent only after `step` has run.
    pub(crate) fn before(mut self, step: impl Fn() + Send + Sync + 'static) -> Self {
        self.before = Some(Hook(Arc::new(step)));
        self
    }

    /// The API's answer for a path it has no object at.
    pub(crate) fn not_found() -> Self {
        Self::json(404, r#"{"detail":"Not found."}"#)
    }

    /// No answer at all, to trigger the client's timeout without a sleep in the test.
    pub(crate) fn never() -> Self {
        Self {
            never: true,
            ..Self::json(200, "")
        }
    }
}

/// A route: the method and path to match, and the answer. A route path with a `?` must
/// match the request's path and query exactly; otherwise the query is ignored. `{base}` in
/// a reply body becomes the server's URL, so a page can link to the next one.
pub(crate) struct Route {
    method: &'static str,
    path: String,
    /// The answers in order: the n-th request gets the n-th, and every later one the last.
    replies: Vec<Reply>,
    /// The requests this route has answered.
    served: AtomicUsize,
}

pub(crate) fn route(method: &'static str, path: &str, reply: Reply) -> Route {
    route_each(method, path, vec![reply])
}

/// A route that answers its requests with `replies` in order, and every request after them
/// with the last: for a test where the server's answer changes between two reads.
pub(crate) fn route_each(method: &'static str, path: &str, replies: Vec<Reply>) -> Route {
    assert!(!replies.is_empty(), "a route needs a reply");
    Route {
        method,
        path: path.to_owned(),
        replies,
        served: AtomicUsize::new(0),
    }
}

/// The method and path of [`any_request`].
const ANY: &str = "*";

/// A route that matches every request, for a test that runs commands it does not script one
/// by one, such as the help examples.
pub(crate) fn any_request(reply: Reply) -> Route {
    route(ANY, ANY, reply)
}

/// A local HTTP/1.1 server on 127.0.0.1. Each connection carries one request, and the
/// answer closes it. The accept thread ends when the server drops: a thread and a socket
/// left behind by each of the suite's servers would run a macOS runner out of threads.
///
/// Every request is checked when the server drops: it matched a route, it carries one
/// `User-Agent: hipc/<version>` and at most one `Authorization` and one of each selector header,
/// and no [`CLUSTER_SECRETS`] key appears in its target, its headers, or its body.
/// An unmatched request still gets a 404, so `hipc` finishes and the test fails on the
/// check; a test that wants a 404 scripts it with [`Reply::not_found`], which pins the
/// path it expects. A run from [`Hipc::against`] also checks each request it sent against
/// the settings it ran with ([`Expected`]).
pub(crate) struct Server {
    url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    /// Set when the server drops, so the accept thread ends at its next connection.
    stop: Arc<std::sync::atomic::AtomicBool>,
    address: std::net::SocketAddr,
}

impl Server {
    pub(crate) fn start(routes: Vec<Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let address = listener.local_addr().expect("a bound address");
        let url = format!("http://127.0.0.1:{}", address.port());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let base = url.clone();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if stopped.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                serve(&stream, &routes, &recorded, &base);
            }
        });
        Self {
            url,
            requests,
            stop,
            address,
        }
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    pub(crate) fn requests(&self) -> Vec<Recorded> {
        snapshot(&self.requests)
    }

    pub(crate) fn only_request(&self) -> Recorded {
        let requests = self.requests();
        assert_eq!(requests.len(), 1, "expected one request, got {requests:#?}");
        requests.into_iter().next().expect("one request")
    }
}

impl Drop for Server {
    /// Fails the test on a request with a wrong fixed header, no route, or a cluster secret
    /// key in its target, a header, or its body. A panic in a drop is the point here: the
    /// check runs for every test without a line in any of them. A test that is already
    /// failing keeps its own message, since a second panic would abort the process.
    fn drop(&mut self) {
        // The connection wakes the accept thread, which sees the flag and ends.
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        drop(std::net::TcpStream::connect(self.address));
        if std::thread::panicking() {
            return;
        }
        for request in self.requests() {
            assert!(
                request.routed,
                "no route for {} {}: script it, or a 404 with `Reply::not_found`",
                request.method, request.target
            );
            request.assert_fixed_headers();
            let body = String::from_utf8_lossy(&request.body);
            let sent = format!("{} {:?} {body}", request.target, request.headers);
            assert_no_phrase(&sent, "a request");
            for secret in CLUSTER_SECRETS {
                let in_header = request
                    .headers
                    .iter()
                    .any(|(_, value)| value.contains(secret));
                assert!(
                    !body.contains(secret) && !request.target.contains(secret) && !in_header,
                    "{} {} sent a cluster secret key",
                    request.method,
                    request.target
                );
            }
        }
    }
}

/// The requests recorded so far.
fn snapshot(requests: &Mutex<Vec<Recorded>>) -> Vec<Recorded> {
    // The list is append-only, so it is still valid after a panic in another test.
    requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn serve(stream: &TcpStream, routes: &[Route], recorded: &Mutex<Vec<Recorded>>, base: &str) {
    let mut reader = BufReader::new(stream);
    let Some(mut request) = read_request(&mut reader) else {
        return;
    };
    let route = routes.iter().find(|r| {
        let target = if r.path.contains('?') {
            request.target.as_str()
        } else {
            request.path()
        };
        let any = r.method == ANY && r.path == ANY;
        any || (r.method == request.method && r.path == target)
    });
    request.routed = route.is_some();
    let reply = route.map_or_else(
        || Reply::json(404, r#"{"detail":"no route"}"#),
        |r| {
            let n = r.served.fetch_add(1, Ordering::Relaxed);
            r.replies[n.min(r.replies.len() - 1)].clone()
        },
    );
    let reply = match &reply.answer {
        Some(Answer(answer)) => answer(&request),
        None => reply,
    };
    recorded
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(request);
    if let Some(Hook(step)) = &reply.before {
        step();
    }
    let mut stream = stream;
    if let Some(raw) = &reply.raw {
        drop(stream.write_all(raw));
    }
    if reply.never || reply.raw.is_some() {
        // Reads until the client closes the connection after its timeout.
        let mut rest = Vec::new();
        drop(reader.read_to_end(&mut rest));
        return;
    }
    let body = match String::from_utf8(reply.body) {
        Ok(text) => text.replace("{base}", base).into_bytes(),
        Err(binary) => binary.into_bytes(),
    };
    let mut head = format!(
        "HTTP/1.1 {} Scripted\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        body.len()
    );
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    drop(stream.write_all(head.as_bytes()));
    drop(stream.write_all(&body));
}

fn read_request(reader: &mut BufReader<&TcpStream>) -> Option<Recorded> {
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
    }
    let mut request = Recorded {
        method,
        target,
        headers,
        body: Vec::new(),
        routed: false,
        received: Instant::now(),
    };
    // The server thread reads the first of a repeated header and never panics: a panic
    // there would end the accept loop, not fail the test.
    request.body = if request
        .values("transfer-encoding")
        .first()
        .is_some_and(|v| v.eq_ignore_ascii_case("chunked"))
    {
        read_chunked(reader)?
    } else {
        let length: usize = request
            .values("content-length")
            .first()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0; length];
        reader.read_exact(&mut body).ok()?;
        body
    };
    request.received = Instant::now();
    Some(request)
}

/// Reads a chunked body (RFC 9112, section 7.1), ignoring chunk extensions and trailers.
fn read_chunked(reader: &mut BufReader<&TcpStream>) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let mut size = String::new();
        reader.read_line(&mut size).ok()?;
        let size = size.trim().split(';').next()?;
        let size = usize::from_str_radix(size, 16).ok()?;
        if size == 0 {
            let mut trailer = String::new();
            reader.read_line(&mut trailer).ok()?;
            return Some(body);
        }
        let mut chunk = vec![0; size + 2];
        reader.read_exact(&mut chunk).ok()?;
        chunk.truncate(size);
        body.extend(chunk);
    }
}

/// A private directory under the system temp directory, removed on drop. It stands in for
/// `$HOME`.
pub(crate) struct Home {
    path: PathBuf,
}

impl Home {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        // Created new with mode 0700, never reused, under a name with a random part: another
        // user who made the path first fails the test instead of owning its home.
        let random = std::hash::BuildHasher::hash_one(
            &std::collections::hash_map::RandomState::new(),
            NEXT.load(Ordering::Relaxed),
        );
        let path = std::env::temp_dir().join(format!(
            "hipc-e2e-{}-{}-{random:016x}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .expect("create a new private temporary home");
        // The umask may have cleared bits of 0700, never added any.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .expect("make the temporary home private");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// `$HOME/.hippius/config.toml`.
    pub(crate) fn config(&self) -> PathBuf {
        self.path.join(".hippius").join("config.toml")
    }

    pub(crate) fn config_text(&self) -> String {
        std::fs::read_to_string(self.config()).unwrap_or_default()
    }

    /// A private config file with `body`, as an older `hip` or a person wrote it.
    pub(crate) fn write_config(&self, body: &str) {
        let config = self.config();
        let dir = config.parent().expect("the config has a directory");
        std::fs::create_dir_all(dir).expect("create the config directory");
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).expect("chmod 700");
        std::fs::write(&config, body).expect("write the config");
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600))
            .expect("chmod 600");
    }

    /// Writes a file with mode 0600 and returns its path.
    pub(crate) fn file(&self, name: &str, contents: &[u8]) -> PathBuf {
        let path = self.path.join(name);
        std::fs::write(&path, contents).expect("write a fixture file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("make the fixture private");
        path
    }
}

impl Drop for Home {
    /// Removes the home, after making each directory in it the owner's again when the first
    /// try fails: a test that failed while a directory was mode 0500 or 0000 leaves no
    /// copy of its token behind.
    fn drop(&mut self) {
        // Best effort: a home left behind under the temp directory fails no test, and a
        // drop that panicked would hide the test's own failure.
        if std::fs::remove_dir_all(&self.path).is_err() {
            reopen(&self.path);
            drop(std::fs::remove_dir_all(&self.path));
        }
    }
}

/// Gives the owner every permission on `dir` and each directory under it.
fn reopen(dir: &Path) {
    drop(std::fs::set_permissions(
        dir,
        std::fs::Permissions::from_mode(0o700),
    ));
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            reopen(&entry.path());
        }
    }
}

/// A `hipc` invocation with the clean environment of [`command`], stdin empty, and stdout
/// and stderr piped.
pub(crate) struct Hipc {
    command: Command,
    stdin: Option<Vec<u8>>,
    /// Where stdout goes instead of a pipe ([`Hipc::stdout_to`]).
    stdout: Option<std::fs::File>,
    /// The server [`Hipc::against`] named, whose requests from this run are checked.
    server: Option<Watched>,
}

/// A server's URL and its request list, shared with the [`Server`].
struct Watched {
    url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

/// The closed loopback origin the environment points `hipc` at when a test names no server,
/// so a test that forgets its server fails with `unavailable` and never reaches
/// `api.hippius.com` ([`crate::dead_port::no_server`]).
pub(crate) use crate::dead_port::no_server;

/// The proxy variables of the developer's shell, which a test must not inherit: a test of an
/// https origin would go through that proxy, and `NO_PROXY` could exempt the default origin
/// from the stand-in proxy of [`Hipc::without_api_url`].
pub(crate) const PROXY_VARIABLES: [&str; 8] = [
    "ALL_PROXY",
    "all_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "NO_PROXY",
    "no_proxy",
];

/// The `hipc` binary with stdin empty and the clean environment every test starts from: no
/// `CI` or `AI_AGENT`, no `HIPPIUS_*`, AWS file, or proxy variables from the parent, `HOME`
/// set to the test home, and `HIPPIUS_API_URL` and `HIPPIUS_TOKEN_ORIGIN` set to
/// [`no_server()`]. A test that runs `hipc` some other way than [`Hipc`] (a terminal, a
/// closed stdout) starts here too, and ends with [`finished`] or [`finish`].
pub(crate) fn command(home: &Home) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hipc"));
    for name in [
        "CI",
        "AI_AGENT",
        "HIPPIUS_TOKEN",
        "HIPPIUS_TOKEN_ORIGIN",
        "HIPPIUS_ACCOUNT",
        "HIPPIUS_PROJECT",
        "HIPPIUS_BILLING",
        "HIPPIUS_PROFILE",
        "HIPPIUS_CONFIG",
        // `hipc s3 configure` writes these files instead of the ones under `HOME`.
        "AWS_CONFIG_FILE",
        "AWS_SHARED_CREDENTIALS_FILE",
    ]
    .into_iter()
    .chain(PROXY_VARIABLES)
    {
        command.env_remove(name);
    }
    // `HIPPIUS_TOKEN` is bound to the same dead origin, so a test that sets it and forgets
    // its server cannot send it to `api.hippius.com` either.
    command
        .env("HOME", home.path())
        .env("HIPPIUS_API_URL", no_server())
        .env("HIPPIUS_TOKEN_ORIGIN", no_server())
        .stdin(Stdio::null());
    command
}

impl Hipc {
    pub(crate) fn new(home: &Home) -> Self {
        Self {
            command: command(home),
            stdin: None,
            stdout: None,
            server: None,
        }
    }

    /// Points `hipc` at a test server with no token, as a person who never logged in runs
    /// it, or one whose token is stored in the config. Each request the run sends there is
    /// checked against the settings it runs with ([`Expected`]), as with [`Hipc::against`].
    pub(crate) fn tokenless(home: &Home, server: &Server) -> Self {
        Self::new(home)
            .env("HIPPIUS_API_URL", server.url())
            .watching(server)
    }

    /// Checks each request this run sends to `server` against the settings it runs with
    /// ([`Expected`]), for a run that names the server some other way, such as `--api-url`
    /// or a profile.
    pub(crate) fn watching(mut self, server: &Server) -> Self {
        self.server = Some(Watched {
            url: server.url.clone(),
            requests: Arc::clone(&server.requests),
        });
        self
    }

    /// [`Hipc::new`] with `HIPPIUS_API_URL` unset, so the origin comes from `--api-url`, the
    /// profile, or the default `https://api.hippius.com`: for a test of that choice. The
    /// command must send nothing to the default origin (a login, a dry run, a refusal), name
    /// a local server through the flag or the profile, or go through a local proxy that
    /// refuses the `CONNECT` (`invites::preview_at_the_default_origin_asks_nothing`).
    ///
    /// `ALL_PROXY`, the first proxy variable the HTTP library reads, points at [`no_server()`],
    /// so a regression that does send to an https origin fails with `unavailable` instead of
    /// reaching `api.hippius.com`. A plain-http loopback origin is reached directly, and a test
    /// that sets its own `ALL_PROXY` replaces this one.
    ///
    /// A server named that way gets only its own checks when it is dropped (a route for
    /// each request, and the fixed headers), not the per-run [`Expected`] check of
    /// [`Hipc::against`]: nothing compares the token and the account header of each request
    /// with the settings of the run.
    pub(crate) fn without_api_url(home: &Home) -> Self {
        Self::new(home)
            .env_remove("HIPPIUS_API_URL")
            .env("ALL_PROXY", no_server())
    }

    /// Points `hipc` at a test server, with the test token in `HIPPIUS_TOKEN`, bound to the
    /// server's origin by `HIPPIUS_TOKEN_ORIGIN`. Each request the run sends there is checked
    /// against the settings it runs with ([`Expected`]).
    pub(crate) fn against(home: &Home, server: &Server) -> Self {
        Self::tokenless(home, server).with_env_token(server.url(), TOKEN)
    }

    /// `token` in `HIPPIUS_TOKEN`, bound to `origin` by `HIPPIUS_TOKEN_ORIGIN`, so `hipc`
    /// sends it to a test server off the default origin.
    pub(crate) fn with_env_token(self, origin: &str, token: &str) -> Self {
        self.env("HIPPIUS_TOKEN", token)
            .env("HIPPIUS_TOKEN_ORIGIN", origin)
    }

    pub(crate) fn env(mut self, name: &str, value: &str) -> Self {
        self.command.env(name, value);
        self
    }

    /// Sets a variable to raw bytes, which need not be UTF-8.
    pub(crate) fn env_bytes(mut self, name: &str, value: &[u8]) -> Self {
        use std::os::unix::ffi::OsStrExt as _;
        self.command.env(name, std::ffi::OsStr::from_bytes(value));
        self
    }

    pub(crate) fn env_remove(mut self, name: &str) -> Self {
        self.command.env_remove(name);
        self
    }

    /// Runs `hipc` in `dir`, where the relative file names of its arguments resolve.
    pub(crate) fn current_dir(mut self, dir: &Path) -> Self {
        self.command.current_dir(dir);
        self
    }

    pub(crate) fn stdin(mut self, bytes: &[u8]) -> Self {
        self.stdin = Some(bytes.to_vec());
        self
    }

    /// Reads stdin from `file`, such as a directory or the config file, which a test cannot
    /// write into a pipe.
    pub(crate) fn stdin_from(mut self, file: std::fs::File) -> Self {
        self.command.stdin(file);
        self
    }

    /// Sends stdout to `file` instead of a pipe, so the run's stdout reads as empty.
    pub(crate) fn stdout_to(mut self, file: std::fs::File) -> Self {
        self.stdout = Some(file);
        self
    }

    pub(crate) fn run(mut self, args: &[&str]) -> Run {
        // Resolved before the run, which may rewrite the config file.
        let expected = self.server.as_ref().map(|server| {
            let sent_before = snapshot(&server.requests).len();
            (server, sent_before, self.expected(args))
        });
        self.command.args(args);
        let stdout = self.stdout.take().map_or_else(Stdio::piped, Stdio::from);
        self.command.stdout(stdout).stderr(Stdio::piped());
        if self.stdin.is_some() {
            self.command.stdin(Stdio::piped());
        }
        let child = self.command.spawn().expect("spawn hipc");
        let run = Run(check_output(finish(child, self.stdin.take(), args)));
        if let Some((server, sent_before, expected)) = expected {
            for request in &snapshot(&server.requests)[sent_before..] {
                expected.check(&server.url, request);
            }
        }
        run
    }

    /// The settings this run resolves, from the same config file, flags, and variables
    /// `hipc` reads, through the same library calls.
    fn expected(&self, args: &[&str]) -> Expected {
        let given = |flag: &'static str, variable: &'static str| -> Option<Option<Given>> {
            if let Some(value) = flag_value(args, flag) {
                let source = Source::Flag(flag);
                return Some(Some(Given { value, source }));
            }
            let value = self.var(variable).ok()?;
            let source = Source::Variable(variable);
            Some(value.map(|value| Given { value, source }))
        };
        // Every `?` is a refusal `hipc` makes before it sends anything.
        let resolved = || {
            let overrides = Overrides {
                profile: given("--profile", "HIPPIUS_PROFILE")?,
                api_url: given("--api-url", "HIPPIUS_API_URL")?,
                account: given("--account", "HIPPIUS_ACCOUNT")?,
                project: given("--project", "HIPPIUS_PROJECT")?,
                billing: given("--billing", "HIPPIUS_BILLING")?,
                token: match self.var("HIPPIUS_TOKEN").ok()? {
                    Some(text) => Some(Token::new(&text, &Source::Variable("HIPPIUS_TOKEN")).ok()?),
                    None => None,
                },
                token_origin: self.var("HIPPIUS_TOKEN_ORIGIN").ok()?.map(|value| Given {
                    value,
                    source: Source::Variable("HIPPIUS_TOKEN_ORIGIN"),
                }),
            };
            let path = hippius::config_path(
                self.var("HIPPIUS_CONFIG").ok()?.map(PathBuf::from),
                self.var("HOME").ok()?.map(PathBuf::from),
            )
            .ok()?;
            let file = ConfigFile::load(path).ok()?;
            let settings = file.settings(&overrides).ok()?;
            let client = Client::new(&settings, Duration::from_secs(1)).ok()?;
            Some(Resolved {
                file,
                overrides,
                settings,
                client,
            })
        };
        Expected(resolved())
    }

    /// The variable `name` as this run sets it, an empty one as unset, as in `hipc`; or the
    /// value that is not UTF-8, which `hipc` refuses.
    fn var(&self, name: &str) -> Result<Option<String>, OsString> {
        let Some((_, Some(value))) = self.command.get_envs().find(|(n, _)| *n == name) else {
            return Ok(None);
        };
        let value = value.to_owned().into_string()?;
        Ok((!value.is_empty()).then_some(value))
    }
}

/// The value of `--flag value` or `--flag=value` in `args`, the last one winning as in
/// clap.
fn flag_value(args: &[&str], flag: &str) -> Option<String> {
    let mut value = None;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if *word == flag {
            value = words.next().map(|v| (*v).to_owned());
        } else if let Some(v) = word.strip_prefix(flag).and_then(|w| w.strip_prefix('=')) {
            value = Some(v.to_owned());
        }
    }
    value
}

/// What each request of one run must carry: the run's settings and the client the library
/// builds from them, or `None` when they do not resolve and `hipc` refuses before sending.
struct Expected(Option<Resolved>);

/// One run's inputs as `hipc` reads them, and what they resolve to.
struct Resolved {
    file: ConfigFile,
    overrides: Overrides,
    settings: Settings,
    client: Client,
}

impl Expected {
    /// On the API origin, `Authorization` with the configured token, or none without one,
    /// and the `X-Hippius-Account` the library gives the path for the selected account
    /// (tenet 12). On another origin, neither (tenet 8).
    fn check(&self, server_url: &str, request: &Recorded) {
        let what = format!("{} {}", request.method, request.target);
        request.assert_fixed_headers();
        let Some(resolved) = &self.0 else {
            panic!("{what} was sent, though the run's settings do not resolve");
        };
        let (settings, client) = (&resolved.settings, &resolved.client);
        let api_origin = settings.api_url.eq_ignore_ascii_case(server_url);
        let authorization = request.header("authorization");
        match (settings.token().is_some(), api_origin) {
            (true, true) => {
                let sent = authorization.unwrap_or_else(|| panic!("{what} has no token"));
                // `Token` keeps its text private; the client identity holds a fingerprint
                // of it, so equal identities on one origin mean the same token.
                assert_eq!(
                    identity(resolved, sent),
                    client.identity(),
                    "{what} has another token"
                );
            }
            _ => assert_eq!(authorization, None, "{what} carries a token"),
        }
        // This pins the wiring: `hipc` sends, once, each header the library computes for the
        // run's selection and this path. It cannot catch a wrong classification, since the
        // expected value comes from the same header functions; the hand-written tables in
        // the library's `every_plan_route_family_gets_the_header_its_class_gives` and
        // `every_route_family_takes_the_selectors_its_class_gives` tests pin each route
        // family to its class.
        let method = Method::parse(&request.method).expect("the server records methods hipc sends");
        let sent = Request::new(method, request.target.clone());
        // The dry-run document names the three headers the library gives the request; a
        // header the request goes out without is present and `null`, never missing.
        let described = if api_origin {
            client
                .describe(&sent)
                .unwrap_or_else(|e| panic!("{what} was sent, though the library refuses it: {e}"))
        } else {
            serde_json::Value::Null
        };
        for (name, field) in [
            ("x-hippius-account", "account_header"),
            ("x-hippius-project", "project_header"),
            ("x-hippius-billing", "billing_header"),
        ] {
            let value = described.get(field);
            if api_origin {
                assert!(value.is_some(), "the dry run of {what} has no `{field}`");
            }
            assert_eq!(
                request.header(name),
                value.and_then(serde_json::Value::as_str),
                "`{name}` of {what}"
            );
        }
    }
}

/// The client identity of the run's inputs with the token in `authorization` instead. A
/// token override always wins, so this resolves through the same library calls as `hipc`.
fn identity(resolved: &Resolved, authorization: &str) -> String {
    let token = authorization
        .strip_prefix("Token ")
        .expect("the fixed-header check before this one holds the Token scheme");
    // `Token::new` trims, so a padded token would hash like the configured one.
    assert_eq!(token, token.trim(), "the token is padded with whitespace");
    let given = &resolved.overrides;
    let overrides = Overrides {
        profile: given.profile.clone(),
        api_url: given.api_url.clone(),
        account: given.account.clone(),
        project: given.project.clone(),
        billing: given.billing.clone(),
        token: Some(Token::new(token, &Source::Argument).expect("a token `hipc` sent is valid")),
        // Bound to the origin the token went to, so a stored token sent there resolves too.
        token_origin: Some(Given {
            value: resolved.settings.api_url.clone(),
            source: Source::Argument,
        }),
    };
    let settings = resolved
        .file
        .settings(&overrides)
        .expect("these inputs resolved once already, and a token override only replaces the token");
    Client::new(&settings, Duration::from_secs(1))
        .expect("the settings resolved once already")
        .identity()
}

/// The checks [`Hipc::run`] makes of every run's output, for a test that runs `hipc` some
/// other way: no test secret on stdout or stderr, and stderr in one voice (tenet 15).
pub(crate) fn check_output(output: Output) -> Output {
    let run = Run(output);
    run.assert_no_secret();
    run.assert_one_voice();
    run.0
}

/// Fails when `text`, the output of a run, holds the test token, a cluster secret key, or
/// four words of an access key phrase in order.
pub(crate) fn assert_no_secret_in(text: &str) {
    assert!(
        !text.contains(TOKEN),
        "the token leaked into output: {text}"
    );
    for secret in CLUSTER_SECRETS {
        assert!(
            !text.contains(secret),
            "a cluster secret key leaked into output: {text}"
        );
    }
    assert_no_phrase(text, "the output");
}

/// The result of one `hipc` run.
pub(crate) struct Run(Output);

impl Run {
    pub(crate) fn code(&self) -> Option<i32> {
        self.0.status.code()
    }

    pub(crate) fn stdout(&self) -> &str {
        std::str::from_utf8(&self.0.stdout).expect("stdout is UTF-8")
    }

    pub(crate) fn stdout_bytes(&self) -> &[u8] {
        &self.0.stdout
    }

    pub(crate) fn stderr(&self) -> &str {
        std::str::from_utf8(&self.0.stderr).expect("stderr is UTF-8")
    }

    pub(crate) fn json(&self) -> Value {
        serde_json::from_slice(&self.0.stdout)
            .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", self.stdout()))
    }

    /// Exit 0, nothing on stderr, and a JSON document on stdout.
    pub(crate) fn ok(&self) -> Value {
        assert_eq!(self.code(), Some(0), "stderr: {}", self.stderr());
        assert_eq!(self.stderr(), "");
        self.json()
    }

    /// [`Run::ok`] without a dry run's `confirmation` and `confirmation_when`
    /// ([`requests_part`]), for a test of the requests a dry run lists.
    pub(crate) fn dry_ok(&self) -> Value {
        requests_part(self.ok())
    }

    /// The tenet 4 failure: this exit code, empty stdout, one JSON object on stderr with
    /// this `code`. Returns the object's `error` field.
    pub(crate) fn fails(&self, exit: i32, code: &str) -> Value {
        assert_eq!(self.code(), Some(exit), "stderr: {}", self.stderr());
        assert_eq!(self.stdout(), "", "stdout must be empty on failure");
        let stderr = self.stderr();
        assert_eq!(stderr.lines().count(), 1, "{stderr}");
        let error: Value = serde_json::from_str(stderr).expect("stderr is one JSON object");
        assert_eq!(error["error"]["code"], code, "{stderr}");
        error["error"].clone()
    }

    /// No test token in any output, whatever the outcome.
    fn assert_no_secret(&self) {
        let all = [&self.0.stdout[..], &self.0.stderr[..]].concat();
        assert_no_secret_in(&String::from_utf8_lossy(&all));
    }

    /// Every run's stderr speaks with one voice (tenet 15). A JSON error object has a
    /// message and a hint that follow the phrasing rules, and an exit-2 or `internal` failure
    /// always has a hint. In text mode every line starts with one of the five labels, apart
    /// from the confirmation prompt and the help a command line without its subcommand
    /// prints after a blank line, and an exit 2 has a `hint:` line.
    fn assert_one_voice(&self) {
        let stderr = String::from_utf8_lossy(&self.0.stderr);
        let lines: Vec<&str> = stderr.lines().collect();
        // A JSON document's `warnings` are text too (tenet 15).
        if let Ok(document) = serde_json::from_slice::<Value>(&self.0.stdout) {
            assert_warnings(&document);
        }
        // In JSON mode the warnings of a result with no place for them are one object on
        // stderr, after any `--debug` lines.
        if let [debug @ .., line] = &lines[..]
            && let Ok(object) = serde_json::from_str::<Value>(line)
            && object.get("warnings").is_some()
            && object.get("error").is_none()
        {
            assert!(
                debug.iter().all(|other| other.starts_with("debug: ")),
                "{stderr}"
            );
            assert_warnings(&object);
            return;
        }
        // In JSON mode the error object is the last line, after any `--debug` lines.
        if let [debug @ .., line] = &lines[..]
            && let Ok(object) = serde_json::from_str::<Value>(line)
            && object.get("error").is_some()
        {
            for other in debug {
                assert!(
                    other.starts_with("debug: "),
                    "a line beside the JSON: {other:?}"
                );
            }
            let error = &object["error"];
            let hint = error["hint"].as_str();
            // Every exit-2 failure has a hint (tenet 4), and so does every `internal` one,
            // which only a bug or a changed API reaches (tenet 15).
            if self.code() == Some(2) || error["code"] == "internal" {
                assert!(hint.is_some_and(|h| !h.is_empty()), "no hint: {line}");
            }
            assert_voice(error["message"].as_str().expect("a message"));
            hint.map(assert_voice);
            assert_warnings(&object);
            return;
        }
        // A command line without its subcommand prints that command's help after a blank
        // line; only the lines before it are hipc's own.
        let own = match stderr.split_once("\n\n") {
            Some((own, help)) if self.code() == Some(2) && help.contains("Usage: hipc") => own,
            _ => stderr.as_ref(),
        };
        let lines: Vec<&str> = own.lines().collect();
        if self.code() == Some(2) {
            assert!(
                lines.iter().any(|line| line.starts_with("hint: ")),
                "an exit-2 failure without a hint: {stderr:?}"
            );
        }
        for line in lines {
            let Some((label, text)) = line.split_once(": ") else {
                panic!("a stderr line without a label: {line:?}");
            };
            match label {
                "error" | "hint" | "note" | "warning" => assert_voice(text),
                "debug" => {}
                _ => assert!(
                    line.contains("continue? [y/N]"),
                    "a stderr line without a label: {line:?}"
                ),
            }
        }
    }
}

/// Checks each text of the top-level `warnings` array a JSON document or error object may
/// carry (tenet 15) against the phrasing rules of [`assert_voice`].
fn assert_warnings(object: &Value) {
    for warning in object
        .get("warnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        assert_voice(warning.as_str().expect("a warning is a text"));
    }
}

/// Checks one message, hint, note, warning, or prompt against the phrasing rules of tenet
/// 15 that a program can check: it starts with a lowercase letter or a literal, not a
/// space; outside the backticked literals there is no period, no `"`, no `'` but an
/// apostrophe between two letters, no second person, no exclamation mark, and none of the
/// words the tenet bans. Inside backticks anything goes:
/// a literal, or the server's own words.
pub(crate) fn assert_voice(text: &str) {
    let first = text.chars().next().expect("a message is not empty");
    assert!(!first.is_whitespace(), "starts with a space: {text:?}");
    assert!(!first.is_ascii_uppercase(), "starts uppercase: {text}");
    assert!(!text.ends_with('.'), "ends with a period: {text}");
    let prose = outside_backticks(text);
    assert!(
        text.matches('`').count().is_multiple_of(2),
        "a backtick is not closed: {text}"
    );
    assert!(
        !prose.contains('"'),
        "double quotes outside backticks: {text}"
    );
    let chars: Vec<char> = prose.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let between_letters = i > 0
            && chars[i - 1].is_alphabetic()
            && chars.get(i + 1).is_some_and(|next| next.is_alphabetic());
        // An apostrophe inside a word (`the API's`) is not a quote; any other one is.
        assert!(
            c != '\'' || between_letters,
            "single quotes outside backticks: {text}"
        );
        // A message is one sentence, so a period belongs only inside a literal.
        assert!(c != '.', "a period outside backticks: {text}");
    }
    assert!(!prose.contains('!'), "an exclamation mark: {text}");
    // "Pay as you go" is the name of the S3 billing mode in every Hippius source
    // (docs.hippius.com/use/console/billing, hippius.com/pricing, swagger), not a word to the
    // reader, so its `you` is not second person.
    let words: Vec<String> = prose
        .to_ascii_lowercase()
        .replace("pay as you go", "")
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_owned)
        .collect();
    for banned in ["you", "your", "please", "sorry", "oops", "fatal", "illegal"] {
        assert!(
            !words.iter().any(|w| w == banned),
            "the word `{banned}`: {text}"
        );
    }
    assert!(
        !prose.contains("something went wrong"),
        "a vague message: {text}"
    );
}

/// The stderr checker refuses a text-mode exit 2 without a hint, and still checks hipc's
/// own lines when help follows them.
#[test]
fn the_stderr_checker_refuses_an_exit_2_without_a_hint() {
    use std::os::unix::process::ExitStatusExt as _;

    let run = |code: i32, stderr: &str| {
        Run(Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        })
    };
    for (code, bad) in [
        (2, "error: no such flag\n"),
        (2, "Error: Bad.\nUsage: hipc\nplain line\n"),
        (2, "Error: Bad.\nhint: run `hipc --help`\n\nUsage: hipc\n"),
        (2, "error: no such flag\n\nUsage: hipc\n"),
        (1, "plain line\n"),
    ] {
        let checked = std::panic::catch_unwind(|| run(code, bad).assert_one_voice());
        assert!(checked.is_err(), "accepted: {bad:?}");
    }
    run(
        2,
        "error: a subcommand is required\nhint: run `hipc --help`\n\nList things\n\nUsage: hipc\n",
    )
    .assert_one_voice();
    run(1, "error: cannot get the VM `v`: the API answered 500\n").assert_one_voice();
}

/// The stderr checker takes the JSON branch only for an error object, and checks the
/// `warnings` of an error object and of a document on stdout.
#[test]
fn the_output_checker_reads_warnings_and_takes_json_only_for_an_error() {
    use std::os::unix::process::ExitStatusExt as _;

    let run = |code: i32, stdout: &str, stderr: &str| {
        Run(Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        })
    };
    let error = r#"{"error":{"code":"conflict","message":"the name is taken"#;
    for (code, stdout, stderr) in [
        (1, "", "42\n"),
        (0, r#"{"warnings":["Bad."]}"#, ""),
        (1, "", &format!(r#"{error}"}},"warnings":["Bad."]}}"#)),
    ] {
        let checked = std::panic::catch_unwind(|| run(code, stdout, stderr).assert_one_voice());
        assert!(checked.is_err(), "accepted: {stdout:?} {stderr:?}");
    }
    run(0, r#"{"warnings":["the file has another link"]}"#, "").assert_one_voice();
    run(1, "", &format!("{error}\"}}}}\n")).assert_one_voice();
}

/// A routed GET of `target` with one user agent, then `headers`.
fn recorded(target: &str, headers: &[(&str, &str)]) -> Recorded {
    let mut all = vec![("user-agent".to_owned(), USER_AGENT.to_owned())];
    all.extend(
        headers
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned())),
    );
    Recorded {
        method: "GET".to_owned(),
        target: target.to_owned(),
        headers: all,
        body: Vec::new(),
        routed: true,
        received: Instant::now(),
    }
}

/// The check on every request refuses a missing, wrong, or repeated fixed header.
#[test]
fn the_fixed_header_check_refuses_a_wrong_or_repeated_header() {
    let token = format!("Token {TOKEN}");
    let fails = |request: Recorded| {
        let checked = std::panic::catch_unwind(|| request.assert_fixed_headers());
        assert!(checked.is_err(), "accepted: {request:?}");
    };
    let path = "/api/compute/vms/";
    let bare = |agent: &[(&str, &str)]| Recorded {
        headers: agent
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect(),
        ..recorded(path, &[])
    };
    fails(bare(&[]));
    fails(bare(&[("user-agent", "curl/8.7.1")]));
    fails(recorded(path, &[("user-agent", USER_AGENT)]));
    fails(recorded(
        path,
        &[("authorization", &format!("Bearer {TOKEN}"))],
    ));
    fails(recorded(
        path,
        &[("authorization", &token), ("authorization", &token)],
    ));
    fails(recorded(
        path,
        &[("x-hippius-account", MEMBER), ("x-hippius-account", MEMBER)],
    ));
    recorded(
        path,
        &[("authorization", &token), ("x-hippius-account", MEMBER)],
    )
    .assert_fixed_headers();
}

/// The check on each request of a run refuses a token or an account header the run's
/// settings do not give, and any request from a run whose settings do not resolve.
#[test]
fn the_run_check_refuses_a_header_the_settings_do_not_give() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let home = Home::new();
    let api = "http://127.0.0.1:1";
    let other = "http://127.0.0.1:2";
    let hipc = || Hipc::new(&home).env("HIPPIUS_API_URL", api);
    let token = format!("Token {TOKEN}");
    let fails = |expected: &Expected, url: &str, request: Recorded| {
        let checked = catch_unwind(AssertUnwindSafe(|| expected.check(url, &request)));
        assert!(checked.is_err(), "accepted: {request:?}");
    };
    let scoped = |headers: &[(&str, &str)]| recorded("/api/compute/vms/", headers);

    let member = hipc().with_env_token(api, TOKEN).expected(&[
        "--account",
        MEMBER,
        "compute",
        "vms",
        "list",
    ]);
    member.check(
        api,
        &scoped(&[("authorization", &token), ("x-hippius-account", MEMBER)]),
    );
    fails(&member, api, scoped(&[("x-hippius-account", MEMBER)]));
    fails(
        &member,
        api,
        scoped(&[
            ("authorization", "Token tok_another_0123456789abcdef"),
            ("x-hippius-account", MEMBER),
        ]),
    );
    fails(&member, api, scoped(&[("authorization", &token)]));
    let personal = recorded(
        "/api/ssh-keys/",
        &[("authorization", &token), ("x-hippius-account", MEMBER)],
    );
    fails(&member, api, personal);
    let drive = recorded("/api/drive/subscription/", &[("authorization", &token)]);
    fails(&member, api, drive);
    fails(
        &member,
        api,
        scoped(&[
            ("authorization", &format!("Token  {TOKEN}")),
            ("x-hippius-account", MEMBER),
        ]),
    );
    fails(
        &member,
        api,
        scoped(&[
            ("authorization", &token),
            ("x-hippius-account", "acct_ffffffffffffffffffff"),
        ]),
    );
    // Another origin gets neither header.
    member.check(other, &scoped(&[]));
    fails(&member, other, scoped(&[("authorization", &token)]));
    fails(&member, other, scoped(&[("x-hippius-account", MEMBER)]));

    let without_token = hipc().expected(&[]);
    without_token.check(api, &scoped(&[]));
    fails(&without_token, api, scoped(&[("authorization", &token)]));

    let unresolved = hipc().expected(&["--account=nobody", "compute", "vms", "list"]);
    assert!(unresolved.0.is_none(), "`nobody` is not an account id");
    fails(&unresolved, api, scoped(&[]));
}

/// A request no route matched fails the test when the server drops, unless the test is
/// already failing, which keeps its own message.
#[test]
fn a_server_refuses_an_unscripted_request_when_it_drops() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let server = Server::start(vec![]);
    Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/compute/vms/"])
        .fails(1, "not_found");
    let payload = catch_unwind(AssertUnwindSafe(|| drop(server)))
        .expect_err("an unrouted request fails the drop");
    let message = payload
        .downcast_ref::<String>()
        .expect("a formatted panic message");
    assert!(
        message.starts_with("no route for GET /api/compute/vms/"),
        "{message}"
    );

    let failed = catch_unwind(|| {
        let server = Server::start(vec![]);
        Hipc::against(&Home::new(), &server)
            .run(&["api", "GET", "/compute/vms/"])
            .fails(1, "not_found");
        panic!("the test's own failure");
    });
    let payload = failed.expect_err("the test panics");
    assert_eq!(
        payload.downcast_ref::<&str>(),
        Some(&"the test's own failure")
    );
}

/// The dry-run document of a command that sends one request, `request`: `dry_run`, and
/// `requests` listing it alone. A `dry_run` in `request` is dropped, as no entry has one.
pub(crate) fn one_request_dry_run(mut request: Value) -> Value {
    if let Value::Object(fields) = &mut request {
        fields.shift_remove("dry_run");
    }
    serde_json::json!({ "dry_run": true, "requests": [request] })
}

/// The dry run `doc` without `confirmation` and `confirmation_when`, for a test of the
/// requests it lists: the examples test checks those two fields of every command's dry run
/// against `hipc schema`.
pub(crate) fn requests_part(mut doc: Value) -> Value {
    if let Value::Object(fields) = &mut doc {
        fields.shift_remove("confirmation");
        fields.shift_remove("confirmation_when");
    }
    doc
}

/// The last request of the dry run `doc`, the write when the command sends one: the request
/// a single-request dry run lists.
pub(crate) fn draft(doc: &Value) -> &Value {
    doc["requests"]
        .as_array()
        .and_then(|requests| requests.last())
        .expect("a dry run with requests")
}

/// `text` without its backticked spans.
fn outside_backticks(text: &str) -> String {
    text.split('`').step_by(2).collect::<Vec<_>>().join(" ")
}

/// A server whose every `GET path` answer is a page linking to a new page, for ever:
/// `body(n)` writes the n-th page served (from 1), naming page `n + 1` as the next, with
/// `{base}` for the server's URL.
pub(crate) fn endless_pages(path: &str, body: fn(usize) -> String) -> Server {
    let served = AtomicUsize::new(0);
    Server::start(vec![route(
        "GET",
        path,
        Reply::from_fn(move |_| {
            let page = served.fetch_add(1, Ordering::Relaxed) + 1;
            Reply::json(200, &body(page))
        }),
    )])
}
