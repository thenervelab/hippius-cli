use serde_json::json;

use std::sync::atomic::AtomicUsize;

use super::*;

fn settings(api_url: &str, account: Selection) -> Settings {
    Settings {
        profile: "default".into(),
        api_url: api_url.into(),
        api_url_source: Source::Flag("--api-url"),
        credential: Credential::Stored(
            crate::config::Token::new("t0k3n", &Source::Argument).unwrap(),
        ),
        account,
        project: None,
        billing: None,
    }
}

fn client(account: Selection) -> Client {
    Client::new(
        &settings("https://api.hippius.com", account),
        Duration::from_secs(5),
    )
    .unwrap()
}

fn no_token() -> Credential {
    Credential::None {
        profile: "default".into(),
        path: "/home/u/.hippius/config.toml".into(),
    }
}

fn member() -> Selection {
    Selection::parse("acct_0123456789abcdef0123", &Source::Argument).unwrap()
}

#[test]
fn a_client_requiring_a_token_refuses_to_send_without_one() {
    let without = |credential: Credential| {
        let mut settings = settings("http://127.0.0.1:9", Selection::Own);
        settings.credential = credential;
        Client::new(&settings, Duration::from_secs(5)).unwrap()
    };
    // Not required: the check passes, and the request would go out without a token.
    assert!(without(no_token()).check_token().is_ok());
    for credential in [
        no_token(),
        Credential::Withheld {
            stored_for: "https://api.hippius.com".into(),
        },
    ] {
        let client = without(credential).requiring_token();
        let error = client.send(&Request::get("/accounts/")).unwrap_err();
        assert_eq!(error.code(), Code::Auth);
        assert!(error.http_status().is_none(), "nothing was sent");
    }
}

#[test]
fn no_token_names_the_profile_and_file_and_who_signs_in() {
    let mut settings = settings("https://api.hippius.com", Selection::Own);
    settings.credential = no_token();
    let error = Client::new(&settings, Duration::from_secs(5))
        .unwrap()
        .requiring_token()
        .check_token()
        .unwrap_err();
    assert_eq!(error.code(), Code::Auth);
    // `no_token` names a file that does not exist.
    assert_eq!(
        error.message(),
        "no API token is configured for `https://api.hippius.com`: the config file \
         `/home/u/.hippius/config.toml` does not exist"
    );
    let hint = error.hint().unwrap();
    assert_eq!(hint, no_token_hint("default"));
    assert!(hint.starts_with("ask the user to run `hipc auth login` in their own terminal"));
    assert!(
        hint.ends_with("with a token created at `https://console.hippius.com/dashboard/settings`")
    );
    assert!(!hint.contains("access"), "{hint}");
    // Another profile is named, so the login stores the token where the run looks.
    assert!(
        no_token_hint("work")
            .starts_with("ask the user to run `hipc auth login --profile work` in their own")
    );
}

#[test]
fn identity_is_the_canonical_origin_and_a_token_fingerprint() {
    let identity = |url: &str, token: Option<&str>| {
        let mut settings = settings(url, Selection::Own);
        settings.credential = token.map_or_else(no_token, |t| {
            Credential::Stored(crate::config::Token::new(t, &Source::Argument).unwrap())
        });
        Client::new(&settings, Duration::from_secs(5))
            .unwrap()
            .identity()
    };
    let canonical = identity("https://api.hippius.com", Some("t0k3n"));
    assert_eq!(
        identity("HTTPS://API.hippius.com:443", Some("t0k3n")),
        canonical
    );
    assert!(
        canonical.starts_with("https://api.hippius.com "),
        "{canonical}"
    );
    assert!(!canonical.contains("t0k3n"), "{canonical}");
    assert_ne!(
        identity("https://api.hippius.com", Some("t0k3m")),
        canonical
    );
    assert_ne!(identity("https://api.hippius.com", None), canonical);
    assert_ne!(identity("https://other.example", Some("t0k3n")), canonical);
    assert_eq!(
        identity("https://api.hippius.com:0443", Some("t0k3n")),
        canonical
    );
    // The whole token, not its last four characters.
    assert_ne!(
        identity("https://api.hippius.com", Some("aaaaaaaaaaaa0123")),
        identity("https://api.hippius.com", Some("bbbbbbbbbbbb0123"))
    );
}

/// The longest URL the HTTP library builds goes through; one byte more is refused. The
/// query a request adds counts, percent-encoded.
#[test]
fn a_url_is_refused_one_byte_past_what_the_http_library_builds() {
    let client = client(Selection::Own);
    let prefix = "https://api.hippius.com/api/x/?q=";
    let at_limit = Request::get(format!(
        "/x/?q={}",
        "a".repeat(MAX_URL_BYTES - prefix.len())
    ));
    assert_eq!(client.url(&at_limit).unwrap().len(), MAX_URL_BYTES);
    ureq::http::Uri::try_from(client.url(&at_limit).unwrap()).expect("the library builds it");
    let over = Request::get(format!(
        "/x/?q={}",
        "a".repeat(MAX_URL_BYTES - prefix.len() + 1)
    ));
    assert_eq!(client.url(&over).unwrap_err().code(), Code::Usage);
    let encoded = Request::get("/x/").query("q", &" ".repeat(MAX_URL_BYTES / 3));
    assert_eq!(client.url(&encoded).unwrap_err().code(), Code::Usage);
}

#[test]
fn debug_of_a_request_hides_query_values_and_the_body() {
    let request = Request::new(Method::Post, "/x/?key=s3cr3t")
        .query("token", "s3cr3t")
        .json(&serde_json::json!({"secret": "s3cr3t"}));
    let debug = format!("{request:?}");
    assert!(!debug.contains("s3cr3t"), "{debug}");
    assert!(debug.contains("query_params: 1"), "{debug}");
}

/// A connection that takes every byte, records how many it was asked to send, and
/// answers nothing.
#[derive(Debug)]
struct Sink(ureq::unversioned::transport::LazyBuffers, Arc<AtomicUsize>);

impl ureq::unversioned::transport::Transport for Sink {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.0
    }

    fn transmit_output(
        &mut self,
        amount: usize,
        _: NextTimeout,
    ) -> std::result::Result<(), ureq::Error> {
        self.1.fetch_add(amount, Ordering::Relaxed);
        Ok(())
    }

    fn await_input(&mut self, _: NextTimeout) -> std::result::Result<bool, ureq::Error> {
        Ok(false)
    }

    fn is_open(&mut self) -> bool {
        true
    }
}

#[test]
fn a_marked_connection_records_the_first_request_byte_and_forwards_the_rest() {
    use ureq::unversioned::transport::Transport as _;

    let written = Arc::new(AtomicBool::new(false));
    let sent = Arc::new(AtomicUsize::new(0));
    let sink: Box<dyn ureq::unversioned::transport::Transport> = Box::new(Sink(
        ureq::unversioned::transport::LazyBuffers::new(16, 16),
        Arc::clone(&sent),
    ));
    let mut marked = MarkedTransport {
        inner: sink,
        written: Arc::clone(&written),
    };
    let timeout = || NextTimeout {
        after: std::time::Duration::from_secs(1).into(),
        reason: ureq::Timeout::Global,
        ..NextTimeout::default()
    };
    // Reading and checking the connection is not writing a request.
    assert!(!marked.await_input(timeout()).unwrap());
    assert!(marked.is_open());
    assert!(!marked.is_tls());
    // The sink's own 16-byte output buffer, not a copy.
    assert_eq!(marked.buffers().output().len(), 16);
    assert!(!written.load(Ordering::Relaxed));
    marked.transmit_output(7, timeout()).unwrap();
    assert!(written.load(Ordering::Relaxed));
    assert_eq!(
        sent.load(Ordering::Relaxed),
        7,
        "the amount reaches the connection"
    );
}

/// A 2xx the command refuses on its content carries what a server error carries: the
/// status, the request id, and the body as `details`. A write is `partial`, a read
/// `none`; a body that is not JSON, or is empty, adds no `details`.
#[test]
fn a_refused_2xx_carries_its_status_request_id_and_body() {
    let answer = |write: bool, body: &[u8]| Response {
        status: 200,
        body: body.to_vec(),
        write,
        action: "rename the peer `12` to `build`".to_owned(),
        request_id: Some("rid".into()),
        date: None,
        token_spellings: Vec::new(),
    };
    let json = answer(true, br#"{"id":"12"}"#)
        .refused("the answer has no `name`")
        .to_json();
    assert_eq!(json["error"]["code"], "internal");
    assert_eq!(
        json["error"]["message"],
        "cannot rename the peer `12` to `build`: the answer has no `name`"
    );
    assert_eq!(json["error"]["http_status"], 200);
    assert_eq!(json["error"]["request_id"], "rid");
    assert_eq!(json["error"]["side_effects"], "partial");
    assert_eq!(json["error"]["retryable"], false);
    assert_eq!(json["error"]["details"], json!({"id": "12"}));
    for body in [b"nope".as_slice(), b""] {
        let json = answer(false, body).refused("why").to_json();
        assert_eq!(json["error"]["side_effects"], "none");
        assert_eq!(json["error"]["http_status"], 200);
        assert_eq!(json["error"]["details"], Value::Null, "{json}");
    }
}

#[test]
fn a_2xx_body_failure_is_maybe_sent_whatever_the_error_says() {
    // On its own, a TLS error is "refused" and an unreachable network is "not sent".
    let tls = || ureq::Error::Io(std::io::Error::other(rustls::Error::DecryptError));
    let unreachable =
        || ureq::Error::Io(std::io::Error::from(std::io::ErrorKind::NetworkUnreachable));
    assert!(matches!(
        how_far(&tls(), Progress::Unwritten),
        Transport::Refused
    ));
    assert!(matches!(
        how_far(&unreachable(), Progress::Unwritten),
        Transport::NotSent
    ));
    // Once a byte went out, neither says the server did not act.
    for error in [tls(), unreachable()] {
        assert!(matches!(
            how_far(&error, Progress::Written),
            Transport::MaybeSent
        ));
    }
    for error in [tls(), unreachable()] {
        let json = transport_error_as(&error, Transport::MaybeSent, true, &attempt()).to_json();
        assert_eq!(json["error"]["side_effects"], "partial", "{error}");
        assert_eq!(json["error"]["retryable"], false, "{error}");
    }
}

#[test]
fn follow_compares_origins_and_refuses_unusable_links() {
    let page = "https://api.hippius.com/api/k/";
    for next in [
        "https://api.hippius.com:443/api/k/?page=2",
        "HTTPS://API.hippius.com/api/k/?page=2",
        "/api/k/?page=2",
    ] {
        assert!(follow(page, next).is_ok(), "{next}");
    }
    // The API's own `http://` links are followed on the page's https origin.
    for next in [
        "http://api.hippius.com/api/k/?page=2",
        "http://API.hippius.com:80/api/k/?page=2",
    ] {
        assert_eq!(
            follow(page, next).unwrap().target,
            "https://api.hippius.com/api/k/?page=2",
            "{next}"
        );
    }
    for next in [
        "https://api.hippius.com/api/k/?page=2 ",
        "/api/k/?page=2\n",
        "https://evil.example/api/k/",
        "http://evil.example/api/k/",
        "http://api.hippius.com:8080/api/k/",
        "http://api.hippius.com:443/api/k/",
        "http://api.hippius.com.evil.example/api/k/",
        "http://api.hippius.com./api/k/",
        "http://api.hippius.com@evil.example/api/k/",
        "http://u@api.hippius.com/api/k/",
        "http://api.hippius.com\\@evil.example/",
    ] {
        assert_eq!(
            follow(page, next).unwrap_err().code(),
            Code::Internal,
            "{next:?}"
        );
    }
    for (next, quoted) in [
        ("?page=2", "`?page=<redacted>`"),
        ("api/k/?page=2", "`api/k/?page=<redacted>`"),
        ("", "``"),
        ("?r=https://x", "`?r=<redacted>`"),
    ] {
        let error = follow(page, next).unwrap_err();
        assert_eq!(error.code(), Code::Internal, "{next:?}");
        assert_eq!(
            error.to_string(),
            format!("the API's `next` link {quoted} is neither a URL nor a path"),
        );
    }
    // Never the other way: an http page does not move onto https, or anywhere else.
    assert_eq!(
        follow("http://127.0.0.1:9/api/k/", "https://127.0.0.1:9/api/k/")
            .unwrap_err()
            .code(),
        Code::Internal
    );
    // A page URL with the default port spelled out is the same origin.
    assert_eq!(
        follow(
            "https://api.hippius.com:443/api/k/",
            "http://api.hippius.com/api/k/?page=2"
        )
        .unwrap()
        .target,
        "https://api.hippius.com/api/k/?page=2"
    );
}

/// A request built from a route keeps its target as `hipc api` takes it, so a hint names
/// the route as a caller types it, and goes to the route's whole path.
#[test]
fn a_route_request_goes_to_its_path_and_names_it_without_the_base() {
    let request = Request::to(&crate::routes::account::MEMBER_DELETE, [&7]);
    assert_eq!(request.method(), Method::Delete);
    assert_eq!(request.target(), "/accounts/members/7/");
    assert_eq!(
        client(Selection::Own).url(&request).unwrap(),
        "https://api.hippius.com/api/accounts/members/7/"
    );
    assert!(under_api_base("/api/x/"));
    assert!(!under_api_base("/apix/") && !under_api_base("/x/api/"));
}

/// A request built from a route takes what its row says of it: a read whose row provisions
/// is a write, as `hipc api` treats the same path, and its paging.
#[test]
fn a_route_request_takes_access_and_paging_from_its_row() {
    use crate::routes::{compute, network};
    let me = Request::to(&network::ME, []);
    assert!(!me.is_read());
    assert_eq!(me.access(), Access::Write);
    let keys = Request::to(&compute::SSH_KEYS, []);
    assert!(keys.is_read());
    assert_eq!(keys.paging(), Some(Paging::Numbered));
}

#[test]
fn paths_join_under_api_once_and_queries_are_encoded() {
    let c = client(Selection::Own);
    assert_eq!(
        c.url(&Request::get("/accounts/")).unwrap(),
        "https://api.hippius.com/api/accounts/"
    );
    assert_eq!(
        c.url(&Request::get("/api/accounts/")).unwrap(),
        "https://api.hippius.com/api/accounts/"
    );
    assert_eq!(
        c.url(&Request::get("/registry/namespaces/check/").query("name", "a b&c/é"))
            .unwrap(),
        "https://api.hippius.com/api/registry/namespaces/check/?name=a%20b%26c%2F%C3%A9"
    );
    assert_eq!(
        c.url(&Request::get("/x/?page=2").query("page_size", "5"))
            .unwrap(),
        "https://api.hippius.com/api/x/?page=2&page_size=5"
    );
    assert_eq!(
        c.url(&Request::get("https://API.hippius.com/api/accounts/"))
            .unwrap(),
        "https://api.hippius.com/api/accounts/"
    );
    assert_eq!(
        c.url(&Request::get("https://api.hippius.com?x")).unwrap(),
        "https://api.hippius.com/?x"
    );
    for bad in ["accounts/", "ftp://x/y", "https:///x", "nope://"] {
        assert_eq!(
            c.url(&Request::get(bad)).unwrap_err().code(),
            Code::Usage,
            "{bad}"
        );
    }
}

/// The account token route is never sent, on any method: the refusal comes before the
/// secret check, so `hipc api` names it rather than ask for a `--reveal` that leads to it.
#[test]
fn the_account_token_route_is_refused_before_the_secret_check() {
    let c = client(Selection::Own);
    for method in [
        Method::Get,
        Method::Head,
        Method::Post,
        Method::Put,
        Method::Patch,
        Method::Delete,
    ] {
        let why = if matches!(method, Method::Get | Method::Head) {
            "its answer is the account's API token"
        } else {
            "the route hands out the account's API token"
        };
        for path in [
            "/auth/token/",
            "/auth/token",
            "/api/auth/token",
            "/auth/token.json",
            "/api/auth/token/?x=1",
        ] {
            let request = Request::new(method, path);
            let error = c.returns_secret(&request).unwrap_err();
            assert_eq!(error.code(), Code::Usage, "{method} {path}");
            assert_eq!(
                error.message(),
                format!("`hipc api` does not send `{method} /api/auth/token/`: {why}"),
                "{method} {path}"
            );
            assert_eq!(
                error.hint(),
                Some(
                    "create a token in the console settings at \
                     `https://console.hippius.com/dashboard/settings`"
                ),
                "{method} {path}"
            );
            assert_eq!(c.route_flags(&request).unwrap_err().code(), Code::Usage);
        }
    }
}

/// A full URL off the configured origin is refused before the account rules, a
/// look-alike host and another port included, and every public check of the request
/// refuses it the same way; the configured origin in another spelling is that origin.
#[test]
fn a_full_url_on_another_origin_is_refused() {
    let c = client(member());
    for url in [
        "https://evil.example/api/compute/vms/",
        "https://api.hippius.com.evil.example/api/compute/vms/",
        "https://api.hippius.com:8443/api/compute/vms/",
        "http://api.hippius.com/api/compute/vms/",
        "http://127.0.0.1:9/api/compute/vms/",
        "https://user@api.hippius.com/api/compute/vms/",
    ] {
        let request = Request::get(url);
        let error = c.resolve(&request).unwrap_err();
        assert_eq!(error.code(), Code::Usage, "{url}");
        assert_eq!(
            error.hint(),
            Some("pass a path such as `/compute/vms/`, or use curl for another host")
        );
        for refused in [
            c.returns_secret(&request).map(|_| ()),
            c.provisions(&request).map(|_| ()),
            c.is_permanent(&request).map(|_| ()),
            c.route_flags(&request).map(|_| ()),
            c.route_of(&request).map(|_| ()),
            c.send(&request).map(|_| ()),
        ] {
            assert_eq!(refused.unwrap_err().code(), Code::Usage, "{url}");
        }
    }
    let error = c
        .resolve(&Request::get("https://evil.example/x/?key=s3cr3t"))
        .unwrap_err();
    assert_eq!(
        error.message(),
        "`https://evil.example/x/?key=<redacted>` is not on the API origin \
         `https://api.hippius.com`: `hipc` sends only there"
    );
    for url in [
        "https://api.hippius.com/api/compute/vms/",
        "HTTPS://API.hippius.com:443/api/compute/vms/",
    ] {
        let resolved = c.resolve(&Request::get(url)).unwrap();
        assert_eq!(
            resolved.account_header.as_deref(),
            Some("acct_0123456789abcdef0123"),
            "{url}"
        );
    }
}

/// The answer comes before the account rules, so `--reveal` can be checked first: a
/// membership on a route that cannot act in another account still gets an answer.
#[test]
fn whether_a_route_issues_a_secret_ignores_the_selected_account() {
    let c = client(member());
    let post = |target: &str| Request::new(Method::Post, target);
    let provision = post("/registry/provision/");
    assert_eq!(
        c.account_header(&provision).unwrap_err().code(),
        Code::AccountScopeUnsupported
    );
    assert!(c.returns_secret(&provision).unwrap());
    assert!(
        !c.returns_secret(&Request::get("/registry/provision/"))
            .unwrap()
    );
    assert!(
        c.returns_secret(&post("https://api.hippius.com/api/registry/provision/?a=1"))
            .unwrap()
    );
    assert!(
        c.returns_secret(&post("https://evil.example/api/registry/provision/"))
            .is_err()
    );
    assert!(c.returns_secret(&post("registry/provision/")).is_err());
}

/// Whether a read provisions depends on the method and the target alone, as the secret
/// answer does: a client in a project and a billing account, which the route refuses
/// once the request is marked a write, still gets the answer; a full URL on the API
/// origin counts, and a foreign origin or a bad target is the same refusal `send` makes.
#[test]
fn whether_a_read_provisions_ignores_the_selection() {
    let c = selecting(member());
    let summary = Request::get("/network/me/");
    assert!(c.provisions(&summary).unwrap());
    let marked = summary.provisions();
    assert_eq!(
        c.url(&marked).unwrap_err().code(),
        Code::BillingScopeUnsupported
    );
    assert!(c.provisions(&marked).unwrap());
    assert!(
        c.provisions(&Request::new(Method::Head, "/network/me"))
            .unwrap()
    );
    assert!(
        !c.provisions(&Request::new(Method::Post, "/network/me/"))
            .unwrap()
    );
    assert!(!c.provisions(&Request::get("/network/peers/")).unwrap());
    assert!(
        c.provisions(&Request::get("https://api.hippius.com/api/network/me/?a=1"))
            .unwrap()
    );
    assert!(
        c.provisions(&Request::get("https://evil.example/api/network/me/"))
            .is_err()
    );
    assert!(c.provisions(&Request::get("network/me/")).is_err());
}

/// The route flag, like the secret check, comes before the account rules, ignores the
/// query, and refuses another origin.
#[test]
fn the_route_flag_ignores_the_account_and_the_query() {
    let c = client(member());
    let put = |target: &str| Request::new(Method::Put, target);
    let storage = put("/v1/operator/nodes/n1/storage/");
    assert_eq!(
        c.account_header(&storage).unwrap_err().code(),
        Code::AccountScopeUnsupported
    );
    let names = |request: &Request| flag_names(&c, request);
    assert_eq!(names(&storage), ["--remove-others"]);
    assert_eq!(
        names(&put(
            "https://api.hippius.com/api/v1/operator/nodes/n1/storage/?force=1"
        )),
        ["--remove-others"]
    );
    assert_eq!(
        names(&Request::new(Method::Post, "/network/isolation/?a=1")),
        ["--change-isolation"]
    );
    let foreign = put("https://evil.example/api/v1/operator/nodes/n1/storage/");
    assert_eq!(c.route_flags(&foreign).unwrap_err().code(), Code::Usage);
    assert_eq!(names(&Request::get("/network/isolation/")), NONE);
    // A Hub delete needs `--yes` alone.
    let repository = Request::new(Method::Delete, "/registry/repositories/app/");
    assert_eq!(names(&repository), NONE);
    assert!(
        c.route_flags(&put("v1/operator/nodes/n1/storage/"))
            .is_err()
    );
}

/// A route table's value is its first row that names the request, slashless and suffixed
/// forms included; a target the client refuses is an error.
#[test]
fn a_route_value_is_the_first_row_that_names_the_request() {
    let c = client(member());
    let table = [
        (Method::Delete, "/api/compute/vms/{id}/", "vm"),
        (Method::Delete, "/api/registry/subscriptions/{n}/", "plan"),
        (Method::Delete, "/api/registry/subscriptions/{id}/", "later"),
    ];
    let value = |method: Method, target: &str| {
        c.route_value(&Request::new(method, target), &table)
            .unwrap()
            .copied()
    };
    assert_eq!(value(Method::Delete, "/compute/vms/vm-1/"), Some("vm"));
    // In any letter case, as a front end that folds it would deliver it.
    assert_eq!(value(Method::Delete, "/Compute/VMS/vm-1/"), Some("vm"));
    assert_eq!(value(Method::Delete, "/compute/vms/vm-1"), Some("vm"));
    assert_eq!(
        value(Method::Delete, "/registry/subscriptions/7.json"),
        Some("plan")
    );
    assert_eq!(
        value(Method::Delete, "/registry/subscriptions/x/"),
        Some("later")
    );
    assert_eq!(value(Method::Get, "/compute/vms/vm-1/"), None);
    assert_eq!(value(Method::Delete, "/compute/vms/"), None);
    assert!(
        c.route_value(&Request::new(Method::Delete, "compute/vms/1/"), &table)
            .is_err()
    );
}

/// A Hub delete is permanent on the API origin, its query ignored; another origin, and
/// any target the client refuses, is an error.
#[test]
fn a_hub_delete_is_permanent_on_the_api_origin_only() {
    let c = client(member());
    let delete = |target: &str| Request::new(Method::Delete, target);
    assert!(
        c.is_permanent(&delete("/registry/repositories/team/app/"))
            .unwrap()
    );
    assert!(
        c.is_permanent(&delete(
            "https://api.hippius.com/api/registry/repositories/app/artifacts/v1/?a=1"
        ))
        .unwrap()
    );
    assert!(
        c.is_permanent(&delete(
            "https://evil.example/api/registry/repositories/app/"
        ))
        .is_err()
    );
    assert!(
        !c.is_permanent(&Request::get("/registry/repositories/app/"))
            .unwrap()
    );
    assert!(
        c.is_permanent(&delete("registry/repositories/app/"))
            .is_err()
    );
}

/// No route flag.
const NONE: [&str; 0] = [];

/// The flags `request` needs, as the caller types them.
fn flag_names(c: &Client, request: &Request) -> Vec<&'static str> {
    c.route_flags(request)
        .unwrap()
        .iter()
        .map(|flag| flag.flag())
        .collect()
}

/// A zone PATCH needs `--public` only when its body sets `origin`, and a member PATCH
/// or a WAF list PATCH needs `--remove-others` only when it sets `grants` or `items`; a
/// body that is not an object fails closed, and no body sets nothing.
#[test]
fn a_body_rule_reads_the_top_level_keys_of_the_body() {
    let c = client(member());
    let patch = |target: &str, body: Option<&str>| {
        let request = Request::new(Method::Patch, target);
        match body {
            Some(body) => request.raw_json(body.as_bytes().to_vec()),
            None => request,
        }
    };
    let names = |request: &Request| flag_names(&c, request);
    for (target, key, other, expected) in [
        ("/cdn/zones/z1/", "origin", "name", "--public"),
        (
            "/accounts/members/7/",
            "grants",
            "can_billing",
            "--remove-others",
        ),
        (
            "/cdn/lists/blocked/",
            "items",
            "description",
            "--remove-others",
        ),
    ] {
        for body in [
            format!(r#"{{"{key}":{{}}}}"#),
            format!(r#"{{"{key}":null}}"#),
            format!(r#"{{"{other}":1,"{key}":[]}}"#),
            "[]".to_owned(),
            "null".to_owned(),
            "\"x\"".to_owned(),
            "{".to_owned(),
        ] {
            assert_eq!(names(&patch(target, Some(&body))), [expected], "{body}");
        }
        for body in [format!(r#"{{"{other}":1}}"#), "{}".to_owned()] {
            assert_eq!(names(&patch(target, Some(&body))), NONE, "{body}");
        }
        // A key below the top level is not the key.
        let nested = format!(r#"{{"{other}":{{"{key}":1}}}}"#);
        assert_eq!(names(&patch(target, Some(&nested))), NONE);
        assert_eq!(names(&patch(target, None)), NONE);
    }
}

/// A Hub namespace publicity PATCH needs `--public` unless its body is an object that sets
/// `public` to `false`: no body, `{}`, and a body that is not an object fail closed, since
/// swagger calls the route a toggle; a `public` below the top level is not the key, and a
/// key given twice counts as its last copy.
#[test]
fn a_publicity_patch_needs_public_unless_it_makes_the_namespace_private() {
    let c = client(member());
    let patch = |body: Option<&str>| {
        let request = Request::new(Method::Patch, "/registry/me/publicity/");
        match body {
            Some(body) => request.raw_json(body.as_bytes().to_vec()),
            None => request,
        }
    };
    for body in [
        r#"{"public":true}"#,
        r#"{"public":"false"}"#,
        r#"{"public":0}"#,
        r#"{"public":null}"#,
        r#"{"public":false,"public":true}"#,
        "[]",
        "null",
        "{",
        "{}",
        r#"{"name":{"public":false}}"#,
    ] {
        assert_eq!(flag_names(&c, &patch(Some(body))), ["--public"], "{body}");
    }
    assert_eq!(flag_names(&c, &patch(None)), ["--public"]);
    for body in [
        r#"{"public":false}"#,
        r#"{"public":true,"public":false}"#,
        r#"{"name":{"public":true},"public":false}"#,
    ] {
        assert_eq!(flag_names(&c, &patch(Some(body))), NONE, "{body}");
    }
    let get = Request::get("/registry/me/publicity/");
    assert_eq!(flag_names(&c, &get), NONE);
}

/// An App Engine launch, compose update, or compose check is refused when its body sets
/// `secrets` at the top level, or is not a JSON object; otherwise it needs no flag.
#[test]
fn an_app_engine_body_with_secrets_is_refused() {
    let c = client(member());
    for (method, target, hint) in [
        (Method::Post, "/compute/app-engine/apps/", "apps create"),
        (
            Method::Put,
            "/compute/app-engine/apps/vm-1/compose/",
            "not in this version",
        ),
        (
            Method::Patch,
            "/compute/app-engine/apps/vm-1/compose",
            "not in this version",
        ),
        (Method::Post, "/compute/app-engine/validate", "validate`"),
    ] {
        let with = |body: &str| Request::new(method, target).raw_json(body.as_bytes().to_vec());
        for body in [
            r#"{"compose":"x","secrets":{}}"#,
            r#"{"secrets":null}"#,
            "[]",
            "{",
        ] {
            let error = c.route_flags(&with(body)).unwrap_err();
            assert_eq!(error.code(), Code::Usage, "{target} {body}");
            assert!(error.message().contains("with `secrets`"), "{target}");
            assert!(error.hint().unwrap().contains(hint), "{target}");
        }
        for body in [r#"{"compose":"x"}"#, r#"{"compose":{"secrets":{}}}"#] {
            assert_eq!(flag_names(&c, &with(body)), NONE, "{target} {body}");
        }
        assert_eq!(flag_names(&c, &Request::new(method, target)), NONE);
    }
}

/// A bucket ACL set needs `--public` and then `--remove-others`, whatever its body.
#[test]
fn a_bucket_acl_set_needs_both_flags_whatever_the_body() {
    let c = client(member());
    for body in [r#"{"policy":{}}"#, r#"{"grants":{}}"#, "{}", "[]"] {
        let request = Request::new(Method::Put, "/objectstore/buckets/b/acl/set/")
            .raw_json(body.as_bytes().to_vec());
        assert_eq!(
            flag_names(&c, &request),
            ["--public", "--remove-others"],
            "{body}"
        );
    }
    let bare = Request::new(Method::Put, "/objectstore/buckets/b/acl/set");
    assert_eq!(flag_names(&c, &bare), ["--public", "--remove-others"]);
}

/// An account deletion request is refused whatever the flags, with a hint that points
/// at the console, and the query does not hide it.
#[test]
fn an_account_deletion_request_is_left_to_the_console() {
    let c = client(member());
    let error = c
        .route_flags(&Request::new(Method::Post, "/account/deletion/?a=1"))
        .unwrap_err();
    assert_eq!(error.code(), Code::Usage);
    assert_eq!(
        error.hint(),
        Some("request the deletion in the console, under Settings, Delete Account")
    );
}

/// A Kubernetes cluster's secrets read, GET or HEAD, is refused whatever the flags, with
/// a hint that points at `clusters kubeconfig`.
#[test]
fn a_cluster_secrets_read_is_left_to_the_kubeconfig_command() {
    let c = client(member());
    for method in [Method::Get, Method::Head] {
        for target in [
            "/kubernetes/clusters/7/secrets/",
            "/kubernetes/clusters/7/secrets",
            "/kubernetes/clusters/7/secrets.json",
            "/kubernetes/clusters/7/secrets.json/",
            "/kubernetes/clusters/7/secrets.api",
            "/kubernetes/clusters/7/secrets/?a=1",
            "/api/kubernetes/clusters/7/secrets/?format=json",
            "https://api.hippius.com/api/kubernetes/clusters/7/secrets/",
        ] {
            let error = c.route_flags(&Request::new(method, target)).unwrap_err();
            assert_eq!(error.code(), Code::Usage, "{method} {target}");
            assert_eq!(
                error.message(),
                format!(
                    "`hipc api` does not send `{method} \
                     /api/kubernetes/clusters/<cluster_id>/secrets/`: its answer holds the \
                     cluster's sealed kubeconfig"
                ),
                "{method} {target}"
            );
            assert_eq!(
                error.hint(),
                Some(
                    "run `hipc compute kubernetes clusters kubeconfig`, which opens the \
                     kubeconfig with the cluster key file and writes it to a new file"
                ),
                "{method} {target}"
            );
        }
    }
}

/// A refusal names the method the request sent and the route the guard comes from, with
/// each placeholder as a person types it: a write that borrows another write's refusal
/// on its route names its own method, and a `body` key the refusal depends on is named.
#[test]
fn a_refusal_names_the_method_sent_and_the_route_of_its_guard() {
    let c = client(Selection::Own);
    let message = |method, target: &str, body: Option<&str>| {
        let mut request = Request::new(method, target);
        if let Some(body) = body {
            request = request.raw_json(body.as_bytes().to_vec());
        }
        c.route_flags(&request).unwrap_err().message().to_owned()
    };
    assert_eq!(
        message(Method::Head, "/kubernetes/clusters/7/secrets/", None),
        "`hipc api` does not send `HEAD /api/kubernetes/clusters/<cluster_id>/secrets/`: \
         its answer holds the cluster's sealed kubeconfig"
    );
    assert_eq!(
        message(Method::Put, "/registry/subscribe/", None),
        "`hipc api` does not send `PUT /api/registry/subscribe/`: it reads no current plan \
         first, so it could start a second paid Hub subscription"
    );
    assert_eq!(
        message(
            Method::Patch,
            "/compute/app-engine/apps/v1/compose/",
            Some(r#"{"secrets":{}}"#)
        ),
        "`hipc api` does not send `PATCH /api/compute/app-engine/apps/<vm_id>/compose/` \
         with `secrets`: the body holds the app's secret values"
    );
    assert_eq!(
        message(Method::Delete, "/auth/token/", None),
        "`hipc api` does not send `DELETE /api/auth/token/`: the route hands out the \
         account's API token"
    );
}

#[test]
fn dry_run_describes_without_header_values() {
    let c = client(member());
    let doc = c
        .describe(&Request::new(Method::Post, "/s3/subscription/").json(&json!({"plan": "pro"})))
        .unwrap();
    assert_eq!(
        doc,
        json!({
            "dry_run": true,
            "method": "POST",
            "url": "https://api.hippius.com/api/s3/subscription/",
            "account_header": "acct_0123456789abcdef0123",
            "project_header": null,
            "billing_header": null,
            "body": {"plan": "pro"}
        })
    );
    assert!(!doc.to_string().contains("t0k3n"));
    let refused = c
        .describe(&Request::get("/drive/subscription/"))
        .unwrap_err();
    assert_eq!(refused.code(), Code::AccountScopeUnsupported);
    let not_json = Request::new(Method::Post, "/x/").raw_json(b"{".to_vec());
    assert_eq!(
        client(Selection::Own).describe(&not_json).unwrap()["body"],
        Value::Null
    );
}

const PROJECT: &str = "proj_0a1b2c3d4e5f60718293";
const BILLING: &str = "bill_9f8e7d6c5b4a39281706";
const MEMBER: &str = "acct_0123456789abcdef0123";

/// A client in the project and billing account of the docs' examples. No route takes
/// both, so every route but the personal ones refuses one of them.
fn selecting(account: Selection) -> Client {
    let mut settings = settings("https://api.hippius.com", account);
    settings.project = ProjectId::parse(PROJECT, &Source::Argument).unwrap();
    settings.billing = BillingId::parse(BILLING, &Source::Argument).unwrap();
    Client::new(&settings, Duration::from_secs(5)).unwrap()
}

/// A client in the project of the docs' example, or in its billing account.
fn in_project(account: Selection) -> Client {
    let mut settings = settings("https://api.hippius.com", account);
    settings.project = ProjectId::parse(PROJECT, &Source::Argument).unwrap();
    Client::new(&settings, Duration::from_secs(5)).unwrap()
}

fn on_billing_account(account: Selection) -> Client {
    let mut settings = settings("https://api.hippius.com", account);
    settings.billing = BillingId::parse(BILLING, &Source::Argument).unwrap();
    Client::new(&settings, Duration::from_secs(5)).unwrap()
}

/// The dry run names each scope header it sends, and `null` for one the request goes
/// out without, whether nothing is selected or the route takes no such header, so its
/// shape never depends on the selection.
#[test]
fn a_dry_run_always_shows_the_three_scope_headers() {
    // Each header as `get` reads it, so a missing key (`None`) fails where `null` is
    // expected.
    fn headers(doc: &Value) -> [Option<&Value>; 3] {
        ["account_header", "project_header", "billing_header"].map(|field| doc.get(field))
    }
    let (null, project, billing) = (Value::Null, json!(PROJECT), json!(BILLING));
    let doc = in_project(Selection::Own)
        .describe(&Request::get("/compute/vms/"))
        .unwrap();
    assert_eq!(headers(&doc), [Some(&null), Some(&project), Some(&null)]);
    let doc = on_billing_account(Selection::Own)
        .describe(&Request::get("/billing/credits/balance/"))
        .unwrap();
    assert_eq!(headers(&doc), [Some(&null), Some(&null), Some(&billing)]);
    let doc = selecting(Selection::Own)
        .describe(&Request::get("/ssh-keys/"))
        .unwrap();
    assert_eq!(headers(&doc), [Some(&null); 3]);
    let doc = client(Selection::Own)
        .describe(&Request::get("/compute/vms/"))
        .unwrap();
    let keys: Vec<&str> = doc
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "dry_run",
            "method",
            "url",
            "account_header",
            "project_header",
            "billing_header",
            "body"
        ]
    );
    assert_eq!(headers(&doc), [Some(&null); 3]);
}

/// The billing account is checked first, so on a route that takes no selector a billing
/// refusal comes before the project's and the account's.
#[test]
fn the_selector_refusals_come_billing_first() {
    let both = selecting(member());
    for path in ["/drive/subscription/", "/compute/vms/"] {
        let error = both.describe(&Request::get(path)).unwrap_err();
        assert_eq!(error.code(), Code::BillingScopeUnsupported, "{path}");
    }
    let error = both
        .describe(&Request::get("/billing/credits/balance/"))
        .unwrap_err();
    assert_eq!(error.code(), Code::ProjectScopeUnsupported);
    let project = in_project(member());
    let error = project
        .describe(&Request::get("/drive/subscription/"))
        .unwrap_err();
    assert_eq!(error.code(), Code::AccountScopeUnsupported);
    let vms = Request::get("/compute/vms/");
    assert_eq!(project.describe(&vms).unwrap()["project_header"], PROJECT);
    assert_eq!(project.billing_header(&vms).unwrap(), None);
    assert_eq!(
        project.account_header(&vms).unwrap().as_deref(),
        Some(MEMBER)
    );
}

/// A membership on a recurring card route is sent beside a billing account, with both
/// headers, and refused without one.
#[test]
fn a_recurring_card_route_takes_the_account_beside_a_billing_account() {
    let stripe = Request::get("/billing/stripe/active-subscription/");
    let doc = on_billing_account(member()).describe(&stripe).unwrap();
    assert_eq!(doc["account_header"], MEMBER);
    assert_eq!(doc["billing_header"], BILLING);
    let error = client(member()).describe(&stripe).unwrap_err();
    assert_eq!(error.code(), Code::AccountScopeUnsupported);
}

/// A server 403 with a selector code passes through with the hint that fixes it.
#[test]
fn a_403_passes_the_selector_codes_through_with_their_hints() {
    for (code, hint) in [
        (
            "project_scope_unsupported",
            "pass `--project default` to act in the Default project",
        ),
        (
            "billing_scope_unsupported",
            "pass `--billing default` to use the Default billing account",
        ),
    ] {
        let body = format!(r#"{{"code":"{code}","detail":"no"}}"#);
        let sent = Attempt {
            project: true,
            billing: true,
            ..attempt()
        };
        let error = status_error(&StatusAnswer {
            attempt: &sent,
            write: false,
            not_done_on_503: false,
            status: 403,
            body: body.as_bytes(),
            retry_after: None,
            request_id: None,
            location: None,
            body_unread: None,
            token_spellings: &[],
            sent_secrets: &[],
        });
        assert_eq!(error.code().as_str(), code);
        assert_eq!(error.code().exit_code(), 2);
        assert_eq!(error.hint(), Some(hint));
    }
}

/// A scope refusal of a header the request did not carry keeps its code (tenet 4), but
/// its hint names no flag value: none would change the answer.
#[test]
fn a_scope_refusal_without_its_header_names_no_flag() {
    for (code, header) in [
        ("account_scope_unsupported", "X-Hippius-Account"),
        ("project_scope_unsupported", "X-Hippius-Project"),
        ("billing_scope_unsupported", "X-Hippius-Billing"),
    ] {
        let body = format!(r#"{{"code":"{code}"}}"#);
        let error = status_error(&StatusAnswer {
            attempt: &attempt(),
            write: false,
            not_done_on_503: false,
            status: 403,
            body: body.as_bytes(),
            retry_after: None,
            request_id: None,
            location: None,
            body_unread: None,
            token_spellings: &[],
            sent_secrets: &[],
        });
        assert_eq!(error.code().as_str(), code);
        let hint = error.hint().unwrap();
        assert!(hint.contains(&format!("no `{header}`")), "{hint}");
        assert!(!hint.contains("pass `--"), "{hint}");
    }
}

/// A scope refusal of a request that carried that header, and only that one, keeps the hint
/// that names the flag value.
#[test]
fn a_scope_refusal_with_its_header_names_the_flag() {
    for (code, member, project, billing) in [
        ("account_scope_unsupported", true, false, false),
        ("project_scope_unsupported", false, true, false),
        ("billing_scope_unsupported", false, false, true),
    ] {
        let body = format!(r#"{{"code":"{code}"}}"#);
        let error = status_error(&StatusAnswer {
            attempt: &Attempt {
                member,
                project,
                billing,
                ..attempt()
            },
            write: false,
            not_done_on_503: false,
            status: 403,
            body: body.as_bytes(),
            retry_after: None,
            request_id: None,
            location: None,
            body_unread: None,
            token_spellings: &[],
            sent_secrets: &[],
        });
        assert_eq!(error.code().as_str(), code);
        let hint = error.hint().unwrap();
        assert!(hint.starts_with("pass `--"), "{code}: {hint}");
    }
}

/// A precondition shows in the dry run only on the request that carries it. `Debug`
/// says whether a request carries a precondition or an idempotency key, never its value,
/// as it shows nothing else the caller gave but the path.
#[test]
fn dry_run_shows_an_if_match_only_when_set() {
    let c = client(Selection::Own);
    let put = Request::new(Method::Put, "/cdn/zones/z1/waf/rules/").json(&json!({}));
    assert!(c.describe(&put).unwrap().get("if_match").is_none());
    let debug = format!("{put:?}");
    assert!(
        debug.contains("if_match: false, idempotency_key: false"),
        "{debug}"
    );
    let put = put.if_match("v-77").idempotency_key("key-88");
    assert_eq!(c.describe(&put).unwrap()["if_match"], "v-77");
    let debug = format!("{put:?}");
    assert!(
        debug.contains("if_match: true, idempotency_key: true"),
        "{debug}"
    );
    assert!(
        !debug.contains("v-77") && !debug.contains("key-88"),
        "{debug}"
    );
}

#[test]
fn debug_of_a_client_hides_the_token() {
    let text = format!("{:?}", client(Selection::Own));
    assert!(!text.contains("t0k3n"), "{text}");
    assert!(text.contains("has_token: true"), "{text}");
}

/// A request to get the VM `vm_1` from `api.hippius.com`, in the token's own account.
fn attempt() -> Attempt<'static> {
    Attempt {
        action: "get the VM `vm_1`".to_owned(),
        host: "api.hippius.com",
        member: false,
        project: false,
        billing: false,
        listed_by: Some("hipc compute vms list"),
        token: TokenFrom::Stored,
    }
}

fn answer(write: bool, status: u16, body: &str) -> Error {
    status_error(&StatusAnswer {
        attempt: &attempt(),
        write,
        not_done_on_503: false,
        status,
        body: body.as_bytes(),
        retry_after: Some("7"),
        request_id: Some("rid".into()),
        location: Some("https://elsewhere/"),
        body_unread: None,
        token_spellings: &[],
        sent_secrets: &[],
    })
}

#[test]
fn every_status_maps_to_its_contract_code() {
    for (status, code) in [
        (400, Code::Validation),
        (401, Code::Auth),
        (402, Code::PaymentRequired),
        (403, Code::Permission),
        (404, Code::NotFound),
        (405, Code::Validation),
        (409, Code::Conflict),
        (412, Code::Conflict),
        (418, Code::Validation),
        (422, Code::Validation),
        (425, Code::Unavailable),
        (429, Code::RateLimited),
        (500, Code::Internal),
        (502, Code::Unavailable),
        (503, Code::Unavailable),
        (504, Code::Unavailable),
        (501, Code::Internal),
        (505, Code::Internal),
        (599, Code::Internal),
        (301, Code::Internal),
        (302, Code::Internal),
        (100, Code::Validation),
    ] {
        assert_eq!(answer(false, status, "").code(), code, "{status}");
    }
}

/// Each account and grant code a 403 may carry keeps its code, a scope refusal its exit 2;
/// any other code is a plain `permission`.
#[test]
fn every_account_refusal_passes_through_and_nothing_else_does() {
    for code in [
        Code::NotAMember,
        Code::AccountScopeUnsupported,
        Code::ProjectScopeUnsupported,
        Code::BillingScopeUnsupported,
        Code::OwnerOnly,
        Code::InsufficientGrant,
        Code::TeamManagementRequired,
        Code::BillingRequired,
    ] {
        let body = format!(r#"{{"code":"{}"}}"#, code.as_str());
        assert_eq!(answer(false, 403, &body).code(), code, "{body}");
    }
    for other in ["usage", "permission", "project_not_found", ""] {
        let body = format!(r#"{{"code":"{other}"}}"#);
        assert_eq!(
            answer(false, 403, &body).code(),
            Code::Permission,
            "{other}"
        );
    }
}

#[test]
fn a_403_passes_account_codes_through() {
    let body = r#"{"code":"owner_only","error":"Only the owner can do this."}"#;
    let error = answer(false, 403, body);
    assert_eq!(error.code(), Code::OwnerOnly);
    let json = error.to_json();
    // The server's text is quoted unchanged, its capital and its period included.
    assert_eq!(
        json["error"]["message"],
        "cannot get the VM `vm_1`: the API says `Only the owner can do this.`"
    );
    assert_eq!(json["error"]["details"]["code"], "owner_only");
    assert_eq!(
        answer(false, 403, r#"{"code":"project_not_found"}"#).code(),
        Code::Permission
    );
}

#[test]
fn message_comes_from_error_detail_or_message_else_the_status() {
    let cases = [
        (r#"{"error":"e"}"#, "the API says `e`"),
        (r#"{"detail":"d"}"#, "the API says `d`"),
        (r#"{"message":"m"}"#, "the API says `m`"),
        (r#"{"error":5,"detail":"d"}"#, "the API says `d`"),
        (r#"{"other":"x"}"#, "the API answered 400"),
        ("not json", "the API answered 400"),
        ("", "the API answered 400"),
        // Django REST framework's field errors, the commonest 400: each
        // field with its messages, the first three in the server's order.
        (
            r#"{"name":["This field is required."]}"#,
            "the API says `name: This field is required.`",
        ),
        (
            r#"{"a":["x","y"],"b":["z"],"c":["w"]}"#,
            "the API says `a: x y; b: z; c: w`",
        ),
        (
            r#"{"a":["x"],"b":["z"],"c":["w"],"d":["v"],"e":["u"]}"#,
            "the API says `a: x; b: z; c: w` and 2 more fields",
        ),
        (
            r#"{"a":["x"],"b":["z"],"c":["w"],"d":["v"]}"#,
            "the API says `a: x; b: z; c: w` and 1 more field",
        ),
        (r#"["Invalid invite."]"#, "the API says `Invalid invite.`"),
        // A field whose errors are not plain text (a nested list item) is left to
        // `details`, and does not count among the others.
        (
            r#"{"rules":[{"port":["bad"]}],"name":["x"]}"#,
            "the API says `name: x`",
        ),
        (r#"{"rules":[{"port":["bad"]}]}"#, "the API answered 400"),
        (r#"{"name":[]}"#, "the API answered 400"),
        ("[1]", "the API answered 400"),
        // A body that is one JSON string is the text itself; a blank one says nothing.
        (r#""Rate limit hit""#, "the API says `Rate limit hit`"),
        (r#""  ""#, "the API answered 400"),
    ];
    for (body, why) in cases {
        assert_eq!(
            answer(false, 400, body).to_string(),
            format!("cannot get the VM `vm_1`: {why}"),
            "{body}"
        );
    }
    // A body that gives no text leaves `details` as the only explanation. The hint holds
    // in both modes: JSON mode already prints `details`, and text mode needs `--json`.
    assert_eq!(
        answer(false, 400, r#"{"other":"x"}"#).hint(),
        Some("check the API's answer in `details`, which `--json` prints")
    );
    assert_eq!(answer(false, 400, r#"{"name":["required"]}"#).hint(), None);
    assert_eq!(answer(false, 400, r#"{"error":"e"}"#).hint(), None);
    // An empty object or list explains nothing, so no hint points at it.
    for empty in ["{}", "[]"] {
        assert_eq!(answer(false, 400, empty).hint(), None, "{empty}");
    }
    assert_eq!(
        answer(false, 400, "not json").to_json()["error"]["details"],
        Value::Null
    );
}

#[test]
fn an_error_body_never_carries_a_one_time_secret_into_details() {
    let error = answer(
        false,
        400,
        concat!(
            r#"{"detail":[{"secret":"s3cr3t","robot_secret":"s3cr3t"}],"secret":"s3cr3t","#,
            r#""credentials":{"password":"s3cr3t"},"access_token":"s3cr3t","#,
            r#""refresh_token":"s3cr3t","ticket":"s3cr3t","setup_key":"s3cr3t","#,
            r#""join_command":"s3cr3t","netbird_setup_key":"s3cr3t","#,
            r#""client_secret":"s3cr3t","encrypted_secret":"s3cr3t","#,
            r#""token":["Invalid invite."],"ok":1}"#
        ),
    );
    let json = error.to_json();
    assert!(!json.to_string().contains("s3cr3t"), "{json}");
    // A bare `token` holds a validation error here, not a secret, so it stays.
    assert_eq!(
        json["error"]["details"],
        serde_json::json!({"detail": [{}], "token": ["Invalid invite."], "ok": 1})
    );
    // Every other spelling of a secret field, in any case and with `_`, `-`, or nothing
    // between its words, is dropped too; a field that only holds such a word is kept, and
    // so is a secret field's list of validation errors, which says why and holds none.
    let error = answer(
        false,
        400,
        concat!(
            r#"{"Password":"s3cr3t","secretKey":"s3cr3t","SECRET-ACCESS-KEY":"s3cr3t","#,
            r#""private_key":"s3cr3t","accessKey":"s3cr3t","api_key":"s3cr3t","#,
            r#""nested":[{"Client-Secret":"s3cr3t"}],"access_key_id":"AKIA","#,
            r#""password_set":true,"secret_access_key":["This field may not be blank."]}"#
        ),
    );
    let json = error.to_json();
    assert!(!json.to_string().contains("s3cr3t"), "{json}");
    assert_eq!(
        json["error"]["details"],
        serde_json::json!({
            "nested": [{}],
            "access_key_id": "AKIA",
            "password_set": true,
            "secret_access_key": ["This field may not be blank."]
        })
    );
    // A spelling no list names is dropped by how it ends: camelCase and acronyms fold
    // like `_`, so `authToken` is `auth_token`, and a name ending in a secret's word is
    // one; a name that only starts with one, or a bare `key`, is kept.
    let error = answer(
        false,
        400,
        concat!(
            r#"{"authToken":"s3cr3t","AuthToken":"s3cr3t","sessionToken":"s3cr3t","#,
            r#""APIKey":"s3cr3t","user_password":"s3cr3t","dbPassphrase":"s3cr3t","#,
            r#""secret_name":"API_KEY","key":"k1","token":"t1"}"#
        ),
    );
    let json = error.to_json();
    assert!(!json.to_string().contains("s3cr3t"), "{json}");
    assert_eq!(
        json["error"]["details"],
        serde_json::json!({"secret_name": "API_KEY", "key": "k1", "token": "t1"})
    );
}

/// Every value a request sends under a secret field is redacted from a field error that
/// repeats it, a number and the strings and numbers nested under the field included; a
/// boolean is not, and a field error of another field is kept as the server wrote it.
#[test]
fn a_field_error_loses_every_value_a_secret_field_sent() {
    let sent = sent_secret_spellings(Some(
        br#"{"password":123456,"api_key":{"id":"key-one-77","parts":["nested-s3cr3t",7.25]},"secret":true,"name":"n"}"#,
    ));
    let error = status_error(&StatusAnswer {
        attempt: &attempt(),
        write: true,
        not_done_on_503: false,
        status: 400,
        body: concat!(
            r#"{"password":["123456 is too common."],"#,
            r#""api_key":["key-one-77 and nested-s3cr3t are taken, 7.25 too; true"],"#,
            r#""name":["123456 is not n"]}"#
        )
        .as_bytes(),
        retry_after: None,
        request_id: None,
        location: None,
        body_unread: None,
        token_spellings: &[],
        sent_secrets: &sent,
    });
    let json = error.to_json();
    assert_eq!(
        json["error"]["details"],
        serde_json::json!({
            "password": ["<redacted> is too common."],
            "api_key": ["<redacted> and <redacted> are taken, <redacted> too; true"],
            "name": ["123456 is not n"]
        })
    );
    let message = json["error"]["message"].as_str().expect("a message");
    for value in ["key-one-77", "nested-s3cr3t", "7.25", "123456 is too"] {
        assert!(!message.contains(value), "{message}");
    }
}

/// A JSON body is written into a buffer of its exact size, measured first, so no smaller
/// copy of it was freed on the way.
#[test]
fn a_json_body_is_written_into_a_buffer_of_its_exact_size() {
    use std::io::Write as _;
    let body = json!({"secrets": {"A": "x".repeat(1000)}, "n": [1, 2.5, "é"]});
    let request = Request::new(Method::Post, "/x/").json(&body);
    let bytes = request.body.as_ref().expect("a body");
    assert_eq!(bytes.as_slice(), serde_json::to_vec(&body).unwrap());
    assert_eq!(bytes.capacity(), bytes.len());
    let mut count = ByteCount(0);
    count.write_all(b"abc").unwrap();
    count.flush().unwrap();
    assert_eq!(count.0, 3);
}

/// A `Deprecation` or `Sunset` header becomes one warning per route, which names the
/// route and the dates the headers hold in their standard forms, and never the server's
/// other text.
#[test]
fn a_deprecated_route_is_one_warning_with_the_dates_its_headers_hold() {
    let warning = |deprecation, sunset| {
        deprecation_warning(Method::Get, "/api/compute/vms/", deprecation, sunset)
    };
    assert_eq!(warning(None, None), None);
    assert_eq!(
        warning(Some("@1782864000"), Some("Fri, 01 Jan 2027 00:00:00 GMT")).as_deref(),
        Some(
            "the API marks `GET /api/compute/vms/` as deprecated as of 2026-07-01, and it \
             may stop answering on 2027-01-01: a newer `hipc` may send another route"
        )
    );
    assert_eq!(
        warning(Some("true"), None).as_deref(),
        Some(
            "the API marks `GET /api/compute/vms/` as deprecated: a newer `hipc` may send \
             another route"
        )
    );
    let odd = warning(Some("@soon <script>"), Some("next week")).expect("a warning");
    assert!(!odd.contains("soon") && !odd.contains("week"), "{odd}");
    assert!(warning(None, Some("Fri, 01 Jan 2027 00:00:00 GMT")).is_some());
}

/// A client keeps the deprecation warnings of its own answers, one per route whatever fills
/// its placeholders, and a path with no row as it is; a take empties them.
#[test]
fn a_client_keeps_one_deprecation_warning_per_route_until_taken() {
    let c = client(Selection::Own);
    let deprecated = |name: &str| (name == "deprecation").then(|| "true".to_owned());
    let none = |_: &str| None;
    c.note_deprecation(Method::Get, "/api/compute/vms/vm_a/", &deprecated);
    c.note_deprecation(Method::Get, "/api/compute/vms/vm_b/", &deprecated);
    c.note_deprecation(Method::Get, "/api/accounts/", &none);
    c.note_deprecation(Method::Get, "/api/unknown/x/", &deprecated);
    let marks = |route: &str| {
        format!("the API marks `GET {route}` as deprecated: a newer `hipc` may send another route")
    };
    assert_eq!(
        c.take_deprecation_warnings(),
        [marks("/api/compute/vms/{vm_id}/"), marks("/api/unknown/x/")]
    );
    assert_eq!(c.take_deprecation_warnings(), Vec::<String>::new());
}

/// A `410 Gone` hints to upgrade `hipc`, whatever its body says, unless it may have been
/// done.
#[test]
fn a_retired_route_hints_to_upgrade() {
    for body in ["", r#"{"detail":"Gone."}"#, r#"{"x":1}"#] {
        let error = answer(false, 410, body);
        assert_eq!(error.code(), Code::Validation, "{body}");
        assert_eq!(error.hint(), Some(RETIRED_HINT), "{body}");
    }
    assert_ne!(answer(true, 400, "").hint(), Some(RETIRED_HINT));
}

#[test]
fn retry_and_side_effects_follow_the_method_and_status() {
    let read_503 = answer(false, 503, "").to_json();
    assert_eq!(read_503["error"]["retryable"], true);
    assert_eq!(read_503["error"]["retry_after_ms"], 7000);
    assert_eq!(read_503["error"]["side_effects"], "none");
    assert_eq!(read_503["error"]["request_id"], "rid");
    let write_503 = answer(true, 503, "").to_json();
    assert_eq!(write_503["error"]["side_effects"], "partial");
    let write_409 = answer(true, 409, "").to_json();
    assert_eq!(write_409["error"]["side_effects"], "none");
    assert_eq!(write_409["error"]["retryable"], false);
    // A redirect answers a write the server received, and `303 See Other` is the usual
    // answer to a POST that was done; only a rejecting 4xx is `none`.
    for status in [301, 302, 303, 307, 308] {
        let redirected = answer(true, status, "");
        assert!(
            matches!(redirected.side_effects(), SideEffects::Partial),
            "{status}"
        );
        assert_eq!(
            redirected.hint(),
            Some("check whether the change was made before running the command again")
        );
        assert!(matches!(
            answer(false, status, "").side_effects(),
            SideEffects::None
        ));
    }
    assert_eq!(answer(false, 429, "").to_json()["error"]["retryable"], true);
    let auth = answer(false, 401, "");
    assert!(auth.hint().unwrap().contains("hipc auth login"));
    let redirect = answer(false, 302, "").to_string();
    assert!(redirect.contains("https://elsewhere/"), "{redirect}");
    let nowhere = status_error(&StatusAnswer {
        attempt: &attempt(),
        write: false,
        not_done_on_503: false,
        status: 301,
        body: b"",
        retry_after: None,
        request_id: None,
        location: None,
        body_unread: None,
        token_spellings: &[],
        sent_secrets: &[],
    });
    assert_eq!(
        nowhere.to_string(),
        "cannot get the VM `vm_1`: the API answered 301 with a redirect and no `Location`"
    );
}

#[test]
fn retry_after_takes_seconds_only() {
    assert_eq!(retry_after_ms(" 3 "), Some(3000));
    assert_eq!(retry_after_ms("Wed, 21 Oct 2015 07:28:00 GMT"), None);
    assert_eq!(retry_after_ms("-1"), None);
    assert_eq!(retry_after_ms(&u64::MAX.to_string()), None);
    // A day is the most taken; past that the value is ignored, not clamped.
    assert_eq!(retry_after_ms("86400"), Some(86_400_000));
    assert_eq!(retry_after_ms("86401"), None);
    assert_eq!(retry_after_ms("18446744073709551"), None);
}

/// The cut falls on a character boundary, counts the characters left, and never cuts a
/// string of the limit or less.
#[test]
fn server_text_is_cut_on_a_character_and_says_how_much_is_left() {
    let limit = "é".repeat(MAX_QUOTED_CHARS);
    assert_eq!(capped(&limit, "!"), format!("`{limit}`"));
    let over = format!("{limit}éa");
    assert_eq!(
        capped(&over, "!"),
        format!("`{limit}` and 2 more characters!")
    );
    assert_eq!(
        capped(&format!("{limit}a"), ""),
        format!("`{limit}` and 1 more character")
    );
}

/// A version no header can carry came from the server, so it is `internal`, refused
/// before anything is sent.
#[test]
fn an_if_match_no_header_can_carry_is_internal() {
    let c = Client::new(
        &settings("http://127.0.0.1:1", Selection::Own),
        Duration::from_secs(1),
    )
    .unwrap();
    let put = Request::new(Method::Put, "/cdn/zones/z1/waf/rules/")
        .json(&json!({}))
        .if_match("v\u{1}7");
    let error = c.send(&put).unwrap_err();
    assert_eq!(error.code(), Code::Internal);
    let message = error.message();
    assert!(message.contains("`If-Match`"), "{message}");
    assert_eq!(error.http_status(), None);
}

/// A dry run refuses what the HTTP library would refuse to build, as the real run does,
/// and a path or target is quoted, so a backtick in it cannot close the literal.
#[test]
fn a_dry_run_refuses_a_target_the_real_run_cannot_send() {
    let c = client(Selection::Own);
    for path in ["/x/?a=b c", "/account/deletion/ ", "/x/\u{7f}/"] {
        let error = c.describe(&Request::get(path)).unwrap_err();
        assert_eq!(error.code(), Code::Usage, "{path:?}");
        assert_eq!(
            error.hint(),
            Some("write the path with only the characters a URL allows"),
            "{path:?}"
        );
    }
    let error = c.describe(&Request::get("x` then run `y")).unwrap_err();
    let message = error.message();
    let quoted = r"`x\u{60} then run \u{60}y` is not an API path";
    assert!(message.starts_with(quoted), "{message}");
    let error = c.describe(&Request::get("/a/../b`x/")).unwrap_err();
    let message = error.message();
    assert!(
        message.starts_with(r"`/a/../b\u{60}x/` has an empty"),
        "{message}"
    );
}

/// Checks `transport_error` for each case: (error, is a write, code, side effects,
/// retryable), with the request's bytes written or not as `progress` says.
fn check_transport(progress: Progress, cases: Vec<(ureq::Error, bool, Code, &str, bool)>) {
    for (error, write, code, side_effects, retryable) in cases {
        let mapped = transport_error(&error, progress, write, &attempt());
        assert_eq!(mapped.code(), code, "{error}");
        let json = mapped.to_json();
        assert_eq!(json["error"]["side_effects"], side_effects, "{error}");
        assert_eq!(json["error"]["retryable"], retryable, "{error}");
    }
}

#[test]
fn a_request_that_never_left_is_retryable_with_no_side_effects() {
    use std::io::ErrorKind;
    // Before any request byte: whatever the error, nothing reached the server. ureq
    // reports a stalled DNS lookup, connect, or handshake as the global timeout.
    check_transport(
        Progress::Unwritten,
        vec![
            (
                ureq::Error::Timeout(ureq::Timeout::Global),
                true,
                Code::Timeout,
                "none",
                true,
            ),
            (
                ureq::Error::Io(std::io::Error::from(ErrorKind::UnexpectedEof)),
                true,
                Code::Unavailable,
                "none",
                true,
            ),
        ],
    );
    // These say so themselves.
    check_transport(
        Progress::Unwritten,
        vec![
            (
                ureq::Error::ConnectionFailed,
                true,
                Code::Unavailable,
                "none",
                true,
            ),
            (
                ureq::Error::HostNotFound,
                true,
                Code::Unavailable,
                "none",
                true,
            ),
            (
                ureq::Error::Io(ErrorKind::ConnectionRefused.into()),
                true,
                Code::Unavailable,
                "none",
                true,
            ),
            (
                ureq::Error::Io(ErrorKind::NetworkUnreachable.into()),
                true,
                Code::Unavailable,
                "none",
                true,
            ),
        ],
    );
}

#[test]
fn a_request_that_may_have_landed_is_partial_and_not_retryable_for_a_write() {
    check_transport(
        Progress::Written,
        vec![
            (
                ureq::Error::Io(std::io::ErrorKind::NetworkUnreachable.into()),
                true,
                Code::Unavailable,
                "partial",
                false,
            ),
            (
                ureq::Error::Tls("alert while reading"),
                true,
                Code::Unavailable,
                "partial",
                false,
            ),
            (
                ureq::Error::Timeout(ureq::Timeout::Global),
                true,
                Code::Timeout,
                "partial",
                false,
            ),
            (
                ureq::Error::Timeout(ureq::Timeout::Global),
                false,
                Code::Timeout,
                "none",
                true,
            ),
            (
                ureq::Error::Io(std::io::Error::other("reset")),
                true,
                Code::Unavailable,
                "partial",
                false,
            ),
            (
                ureq::Error::BodyStalled,
                false,
                Code::Unavailable,
                "none",
                true,
            ),
            (
                ureq::Error::BodyExceedsLimit(1),
                true,
                Code::Internal,
                "partial",
                false,
            ),
        ],
    );
}

#[test]
fn tls_proxy_and_url_failures_are_not_retryable_and_sent_nothing() {
    check_transport(
        Progress::Unwritten,
        vec![
            (
                ureq::Error::Tls("bad certificate"),
                true,
                Code::Unavailable,
                "none",
                false,
            ),
            (
                ureq::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer),
                )),
                true,
                Code::Unavailable,
                "none",
                false,
            ),
            (
                ureq::Error::TlsRequired,
                true,
                Code::Unavailable,
                "none",
                false,
            ),
            (
                ureq::Error::ConnectProxyFailed("refused".into()),
                true,
                Code::Unavailable,
                "none",
                false,
            ),
            (
                ureq::Error::InvalidProxyUrl,
                true,
                Code::Unavailable,
                "none",
                false,
            ),
            (
                ureq::Error::RequireHttpsOnly("x".into()),
                true,
                Code::Unavailable,
                "none",
                false,
            ),
            (
                ureq::Error::BadUri("x".into()),
                true,
                Code::Internal,
                "none",
                false,
            ),
        ],
    );
}

#[test]
fn ambiguous_api_paths_are_refused_before_anything_is_sent() {
    let c = client(member());
    for bad in [
        "/ssh-keys/../compute/vms/",
        "/ssh-keys/./x/",
        "/compute/%2e%2e/drive/",
        "/compute/%2E/x/",
        "/compute/vms%2F1/",
        "/compute/vms%5c1/",
        "/compute\\vms/",
        "/..",
        "//compute/vms/",
        "/registry//keys/",
        "https://api.hippius.com/api/registry//keys/",
        "https://api.hippius.com/api/ssh-keys/../drive/",
    ] {
        let error = c.url(&Request::get(bad)).unwrap_err();
        assert_eq!(error.code(), Code::Usage, "{bad}");
    }
    for fine in [
        "/compute/vms/a.b/",
        "/compute/vms/...x/",
        "/compute/x/?next=../y&u=https://z",
        "/compute/x/?q=%2e",
    ] {
        assert!(c.url(&Request::get(fine)).is_ok(), "{fine}");
    }
    // Another origin is refused whatever its path.
    assert_eq!(
        c.url(&Request::get("https://other.example/a/../b"))
            .unwrap_err()
            .code(),
        Code::Usage
    );
}

#[test]
fn a_url_inside_a_query_does_not_make_a_path_a_url() {
    let c = client(Selection::Own);
    assert_eq!(
        c.url(&Request::get("/x/?next=https://y/z")).unwrap(),
        "https://api.hippius.com/api/x/?next=https://y/z"
    );
}

#[test]
fn response_document_refuses_an_empty_or_null_body() {
    for body in [&b""[..], b"  \n", b"null"] {
        for (write, side_effects) in [(false, "none"), (true, "partial")] {
            let response = Response {
                status: 200,
                body: body.to_vec(),
                write,
                action: "read the thing".to_owned(),
                request_id: None,
                date: None,
                token_spellings: Vec::new(),
            };
            let error = response.document().unwrap_err();
            assert_eq!(error.code(), Code::Internal);
            assert_eq!(error.to_json()["error"]["side_effects"], side_effects);
        }
    }
    let response = Response {
        status: 200,
        body: b"[]".to_vec(),
        write: false,
        action: "read the thing".to_owned(),
        request_id: None,
        date: None,
        token_spellings: Vec::new(),
    };
    assert_eq!(response.document().unwrap(), serde_json::json!([]));
}

#[test]
fn response_json_keeps_large_numbers_and_rejects_non_json() {
    let ok = Response {
        status: 200,
        body: br#"{"paid_per_month":99000000000000000000}"#.to_vec(),
        write: false,
        action: "read the thing".to_owned(),
        request_id: None,
        date: None,
        token_spellings: Vec::new(),
    };
    assert_eq!(
        ok.json().unwrap().to_string(),
        r#"{"paid_per_month":99000000000000000000}"#
    );
    assert_eq!(ok.status(), 200);
    let empty = Response {
        status: 204,
        body: b" \n".to_vec(),
        write: false,
        action: "read the thing".to_owned(),
        request_id: None,
        date: None,
        token_spellings: Vec::new(),
    };
    assert_eq!(empty.json().unwrap(), Value::Null);
    let html = Response {
        status: 200,
        body: b"<html>".to_vec(),
        write: false,
        action: "read the thing".to_owned(),
        request_id: None,
        date: None,
        token_spellings: Vec::new(),
    };
    let error = html.json().unwrap_err();
    assert_eq!(error.code(), Code::Internal);
    assert_eq!(error.to_json()["error"]["side_effects"], "none");
    assert_eq!(error.http_status(), Some(200));
    assert_eq!(html.bytes(), b"<html>");
    let written = Response {
        status: 201,
        body: b"<html>".to_vec(),
        write: true,
        action: "read the thing".to_owned(),
        request_id: None,
        date: None,
        token_spellings: Vec::new(),
    };
    let error = written.json().unwrap_err();
    assert_eq!(error.to_json()["error"]["side_effects"], "partial");
    assert_eq!(error.to_json()["error"]["retryable"], false);
    let debug = format!("{written:?}");
    assert!(
        debug.contains("body_len: 6") && !debug.contains("html"),
        "{debug}"
    );
}
