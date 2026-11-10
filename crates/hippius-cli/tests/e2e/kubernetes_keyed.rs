//! `hipc compute kubernetes` with the cluster key file: `clusters kubeconfig`, `node-pools
//! scale`, and `nodes replace`, against known-answer key files and drops
//! (`common::CLUSTER_KEY`). The cluster is `42`, the id the drops are signed for.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use base64ct::{Base64, Encoding as _};
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::{Value, json};

use crate::common::{
    self, CLUSTER_KEY, CLUSTER_SECRETS, Hipc, Home, MEMBER, OTHER_KEY, Recorded, Reply, Route, Run,
    Server, route, route_each,
};

/// The list of test secrets holds every secret key of both key files, so the leak checks
/// cover each key a test loads.
#[test]
fn the_secret_list_holds_every_secret_key_of_the_key_files() {
    for file in [CLUSTER_KEY, OTHER_KEY] {
        let file: Value = serde_json::from_str(file).expect("the key fixture is JSON");
        for field in ["sign_secret_key", "box_secret_key"] {
            let secret = file[field].as_str().expect("a secret key");
            assert!(CLUSTER_SECRETS.contains(&secret), "{field}");
        }
    }
}

/// The public halves of [`CLUSTER_KEY`], as the cluster records them.
const SIGN_PUBLIC: &str = "VCgh6VKd0a0kO/3/XGQKZRlLSBaYemUN6GHXzp5BemM=";
const BOX_PUBLIC: &str = "B6N8vBQgk8i3VdwbEOhstCY3StFqqFPtC9/AsrhtHHw=";

/// The guest key that signed the vector drops, and another one.
const GUEST: &str = "2inpWwLgD/oVZFd1+x0roiKhlDOV7qBrlOLAV7e+adA=";
const OTHER_GUEST: &str = "p/z33O1VMdWsOFzHvaGk630A1iSKf4+9jbvd9zoh0qA=";

/// The vector kubeconfig, and its drop for the cluster 42.
const KUBECONFIG: &str = "apiVersion: v1\nkind: Config\nclusters:\n- name: m0\n  cluster:\n    \
                          certificate-authority-data: Q0E=\n    server: \
                          https://203.0.113.10:6443\ncurrent-context: m0\n";
const KUBECONFIG_CIPHERTEXT: &str = "bTznQo66nmdAIkFNAD1I39+e5osoMThoZYz3OTlZby2ZZdEy8DN676jADwsFO\
    WK1EZNEDO5MzlSsGkkPw3qgd5bOEol2oLoSL7Bs3E/7KsoiIQev8ITgt42x7+DCBU0cM7Rxy6BN3gasC8an8vwYo9Eh\
    xiaPhpw0GlFQlCeF6ElDgKcdSwju1QYlNy2xUP0MDhJ3S4sQASFwHDIscP3y1J8B+C4l4etEqIRRaLisJNMxKItf0s\
    E4/Y1kbHmQxJVPxcTooKnkPmLB9es=";
const KUBECONFIG_SIGNATURE: &str =
    "MfGE71Br/NFmaEal6U93MGk2tDY9W+6pv6kLO4cPwdOlYGBC+3b9acFYMF/HTfTwXgrJkeaaAYz5+wohCZXyBA==";

/// The worker pool's `signing_params`, its keys in sorted order so the test can write the
/// canonical envelope by hand.
const PARAMS: &str = r#"{"flavor":"medium","image":"ubuntu","k8s_version":"v1.34.3+rke2r1","labels":{},"pool":"workers","taints":[]}"#;

fn cluster(guest: Option<&str>) -> Value {
    json!({
        "id": 42,
        "name": "prod",
        "status": "running",
        "client_sign_pubkey": SIGN_PUBLIC,
        "client_box_pubkey": BOX_PUBLIC,
        "guest_sign_pubkey": guest,
    })
}

fn secrets(guest: &str) -> Value {
    json!({
        "guest_sign_pubkey": guest,
        "drops": [{
            "id": 1,
            "kind": "kubeconfig",
            "operation": null,
            "ciphertext": KUBECONFIG_CIPHERTEXT,
            "signature": KUBECONFIG_SIGNATURE,
            "created_at": "2026-10-01T00:00:00Z"
        }]
    })
}

fn pools() -> Value {
    let params: Value = serde_json::from_str(PARAMS).expect("the params are JSON");
    json!([
        {
            "id": 1,
            "name": "control-plane",
            "role": "control_plane",
            "flavor": "large",
            "signing_params": null,
            "nodes": [{ "name": "m0", "state": "ready" }]
        },
        {
            "id": 3,
            "name": "workers",
            "role": "worker",
            "flavor": "medium",
            "signing_params": params,
            "nodes": [
                { "name": "work-1", "state": "ready" },
                { "name": "work-2", "state": "ready" }
            ]
        }
    ])
}

fn answer(status: u16, value: &Value) -> Reply {
    Reply::json(status, &value.to_string())
}

fn cluster_route(method: &'static str, suffix: &str, reply: Reply) -> Route {
    route(
        method,
        &format!("/api/kubernetes/clusters/42/{suffix}"),
        reply,
    )
}

/// The routes of the three commands, with `operations` as the cluster's operations.
fn routes(operations: &Value) -> Vec<Route> {
    vec![
        cluster_route("GET", "", answer(200, &cluster(Some(GUEST)))),
        cluster_route("GET", "secrets/", answer(200, &secrets(GUEST))),
        cluster_route("GET", "node-pools/", answer(200, &pools())),
        cluster_route("GET", "operations/", answer(200, operations)),
        cluster_route(
            "POST",
            "node-pools/3/scale/",
            answer(200, &json!({ "pool": { "id": 3 }, "operations": [] })),
        ),
        cluster_route(
            "POST",
            "nodes/work-1/replace/",
            answer(
                202,
                &json!({ "id": 20, "kind": "add_node", "state": "queued" }),
            ),
        ),
    ]
}

/// A private home with the key file at `hippius-cluster-prod.key`.
fn home() -> Home {
    let home = Home::new();
    home.file("hippius-cluster-prod.key", CLUSTER_KEY.as_bytes());
    home
}

fn key_path(home: &Home) -> String {
    home.path()
        .join("hippius-cluster-prod.key")
        .to_string_lossy()
        .into_owned()
}

/// `hipc compute kubernetes <args>` in `home`, against `server`.
fn hipc(home: &Home, server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["compute", "kubernetes"];
    argv.extend(args);
    Hipc::against(home, server).run(&argv)
}

fn kubeconfig_args<'a>(key: &'a str, output: &'a str) -> Vec<&'a str> {
    vec![
        "clusters",
        "kubeconfig",
        "42",
        "--cluster-key-file",
        key,
        "--output-file",
        output,
    ]
}

fn message(error: &Value) -> &str {
    error["message"].as_str().expect("the message is a string")
}

/// Asserts that `op` is one signed `add_node` as hipc sends it: its four public fields, and a
/// signature by the key file's signing key over the canonical envelope of `params`.
fn verify(op: &Value, params: &str) {
    let uuid = op["request_uuid"].as_str().expect("a request id");
    let iat = op["issued_at"].as_u64().expect("an issue time");
    let exp = op["expires_at"].as_u64().expect("an expiry");
    assert_eq!(exp, iat + 3000);
    assert_eq!(uuid.len(), 36, "{uuid}");
    assert_eq!(&uuid[14..15], "4", "a v4 request id: {uuid}");
    let envelope = format!(
        r#"{{"cluster":"42","dom":"hippius-k8s-op","exp":{exp},"iat":{iat},"kind":"add_node","kv":1,"params":{params},"req":"{uuid}","v":1}}"#
    );
    let mut public = [0_u8; 32];
    Base64::decode(SIGN_PUBLIC, &mut public).expect("the public key is base64");
    let mut signature = [0_u8; 64];
    Base64::decode(
        op["signature"].as_str().expect("a signature"),
        &mut signature,
    )
    .expect("the signature is 64 bytes of base64");
    VerifyingKey::from_bytes(&public)
        .expect("the public key is a point")
        .verify_strict(envelope.as_bytes(), &Signature::from_bytes(&signature))
        .expect("the signature verifies over the envelope");
    let fields: Vec<&String> = op.as_object().expect("an object").keys().collect();
    assert_eq!(
        fields,
        ["request_uuid", "issued_at", "expires_at", "signature"],
        "only the four public fields are sent"
    );
}

#[test]
fn kubeconfig_is_verified_opened_and_written_to_a_new_private_file() {
    let (home, server) = (home(), Server::start(routes(&json!([]))));
    let output = home.path().join("prod.kubeconfig");
    let key = key_path(&home);
    let out = output.to_string_lossy();
    let run = hipc(&home, &server, &kubeconfig_args(&key, &out));
    assert_eq!(
        run.ok(),
        json!({ "id": 42, "name": "prod", "output_file": out, "bytes": KUBECONFIG.len() })
    );
    assert!(!run.stdout().contains("certificate-authority-data"));
    assert_eq!(std::fs::read_to_string(&output).unwrap(), KUBECONFIG);
    let mode = std::fs::metadata(&output).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(
        server.calls(),
        [
            "GET /api/kubernetes/clusters/42/",
            "GET /api/kubernetes/clusters/42/secrets/"
        ]
    );
    // A second run finds the file and stops before anything is sent.
    let error = hipc(&home, &server, &kubeconfig_args(&key, &out)).fails(2, "usage");
    assert!(
        message(&error).contains("hipc never overwrites it"),
        "{error}"
    );
    assert_eq!(server.requests().len(), 2);
    // Text mode prints the same document as a table; the kubeconfig stays in the file.
    let text = home.path().join("text.kubeconfig");
    let mut argv = vec!["--output", "text", "compute", "kubernetes"];
    let text_out = text.to_string_lossy();
    argv.extend(kubeconfig_args(&key, &text_out));
    let run = Hipc::against(&home, &server).run(&argv);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(!run.stdout().contains("certificate-authority-data"));
}

#[test]
fn kubeconfig_refuses_an_output_file_it_would_overwrite_before_anything_is_sent() {
    let (home, server) = (home(), Server::start(routes(&json!([]))));
    let key = key_path(&home);
    let taken = home.file("taken", b"old");
    std::os::unix::fs::symlink(home.path().join("nowhere"), home.path().join("link")).unwrap();
    let link = home.path().join("link");
    let missing_dir = home.path().join("no").join("k");
    for (output, says) in [
        (taken.to_string_lossy().into_owned(), "never overwrites it"),
        (link.to_string_lossy().into_owned(), "never overwrites it"),
        (
            "-".to_owned(),
            "the kubeconfig is written only to a file, never to stdout",
        ),
        (
            missing_dir.to_string_lossy().into_owned(),
            "the directory of that path does not exist",
        ),
    ] {
        let error = hipc(&home, &server, &kubeconfig_args(&key, &output)).fails(2, "usage");
        assert!(message(&error).contains(says), "{output}: {error}");
    }
    assert_eq!(std::fs::read(&taken).unwrap(), b"old");
    assert!(server.requests().is_empty());
}

/// Key files `hipc` refuses, written in `home`, each with what its refusal says.
fn bad_key_files(home: &Home) -> Vec<(PathBuf, &'static str)> {
    let bad = |name: &str, text: &str, mode: u32| -> PathBuf {
        let path = home.file(name, text.as_bytes());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("chmod a fixture");
        path
    };
    let with = |field: &str, value: Value| {
        let mut file: Value = serde_json::from_str(CLUSTER_KEY).expect("the key fixture is JSON");
        file[field] = value;
        file.to_string()
    };
    let link = home.path().join("link.key");
    std::os::unix::fs::symlink(key_path(home), &link).expect("link a fixture");
    vec![
        (home.path().join("missing.key"), "does not exist"),
        (link, "is a symlink"),
        (
            bad("open.key", CLUSTER_KEY, 0o644),
            "has permission bits for the group or others",
        ),
        (
            bad("group.key", CLUSTER_KEY, 0o640),
            "has permission bits for the group or others",
        ),
        (
            bad("ssh.key", "-----BEGIN OPENSSH PRIVATE KEY-----", 0o600),
            "is not a JSON object",
        ),
        (
            bad("kind.key", &with("kind", json!("other")), 0o600),
            "has no `kind` of `hippius-k8s-cluster-key`",
        ),
        (
            bad("version.key", &with("version", json!(2)), 0o600),
            "is not of version `1`",
        ),
        (
            bad("b64.key", &with("box_secret_key", json!("%%%")), 0o600),
            "has no `box_secret_key` of 32 bytes",
        ),
        (
            bad(
                "damaged.key",
                &with("sign_public_key", json!(BOX_PUBLIC)),
                0o600,
            ),
            "is damaged",
        ),
    ]
}

/// Each refused key file is `usage` before anything is sent, for each command, and no
/// message quotes the file.
#[test]
fn a_bad_key_file_is_usage_with_zero_requests() {
    let (home, server) = (home(), Server::start(routes(&json!([]))));
    let out = home.path().join("k").to_string_lossy().into_owned();
    for (path, says) in bad_key_files(&home) {
        let key = path.to_string_lossy().into_owned();
        for args in [
            kubeconfig_args(&key, &out),
            vec![
                "node-pools",
                "scale",
                "42",
                "3",
                "--count",
                "4",
                "--cluster-key-file",
                &key,
                "--yes",
            ],
            vec![
                "nodes",
                "replace",
                "42",
                "work-1",
                "--cluster-key-file",
                &key,
                "--yes",
            ],
            // A dry run reads the key file too.
            vec![
                "nodes",
                "replace",
                "42",
                "work-1",
                "--cluster-key-file",
                &key,
                "--dry-run",
            ],
        ] {
            let error = hipc(&home, &server, &args).fails(2, "usage");
            assert!(message(&error).contains(says), "{args:?}: {error}");
            assert!(
                message(&error).starts_with("the cluster key file `"),
                "the message names the file: {error}"
            );
        }
    }
    let error = hipc(
        &home,
        &server,
        &kubeconfig_args(&home.path().join("open.key").to_string_lossy(), &out),
    )
    .fails(2, "usage");
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .starts_with("run `chmod 600 "),
        "{error}"
    );
    assert!(server.requests().is_empty());
    assert!(!Path::new(&out).exists());
}

/// A key file that parses but is another cluster's is `usage` after the read of the cluster,
/// and nothing else is sent.
#[test]
fn another_clusters_key_file_is_usage_after_the_cluster_read() {
    let (home, server) = (home(), Server::start(routes(&json!([]))));
    let other = home.file("other.key", OTHER_KEY.as_bytes());
    let key = other.to_string_lossy().into_owned();
    let out = home.path().join("k").to_string_lossy().into_owned();
    for args in [
        kubeconfig_args(&key, &out),
        vec![
            "node-pools",
            "scale",
            "42",
            "3",
            "--count",
            "4",
            "--cluster-key-file",
            &key,
        ],
        vec![
            "nodes",
            "replace",
            "42",
            "work-1",
            "--cluster-key-file",
            &key,
        ],
    ] {
        let error = hipc(&home, &server, &args).fails(2, "usage");
        assert!(
            message(&error).ends_with(
                "is not the key of the Kubernetes cluster `prod` (`42`): its public keys are not \
                 the ones the cluster was created with"
            ),
            "{args:?}: {error}"
        );
    }
    assert!(
        server
            .calls()
            .iter()
            .all(|r| r == "GET /api/kubernetes/clusters/42/")
    );
    assert_eq!(server.requests().len(), 3);
    assert!(!Path::new(&out).exists());
}

/// A drop the cluster's guest key did not sign is refused, `internal`, and nothing is
/// decrypted or written; a cluster that has not published its key yet is `not_found`.
#[test]
fn a_drop_that_does_not_verify_writes_nothing() {
    let home = home();
    let mut other = cluster(Some(OTHER_GUEST));
    other["name"] = json!("prod");
    let server = Server::start(vec![
        cluster_route("GET", "", answer(200, &other)),
        cluster_route("GET", "secrets/", answer(200, &secrets(OTHER_GUEST))),
    ]);
    let out = home.path().join("k").to_string_lossy().into_owned();
    let error = hipc(&home, &server, &kubeconfig_args(&key_path(&home), &out)).fails(1, "internal");
    assert_eq!(
        message(&error),
        "the kubeconfig of the Kubernetes cluster `prod` (`42`) is not signed by the cluster's \
         guest key, and it was not decrypted, so it was refused"
    );
    assert_eq!(error["side_effects"], "none");
    assert!(!Path::new(&out).exists());
    let server = Server::start(vec![
        cluster_route("GET", "", answer(200, &cluster(None))),
        cluster_route("GET", "secrets/", answer(200, &json!({ "drops": [] }))),
    ]);
    let error =
        hipc(&home, &server, &kubeconfig_args(&key_path(&home), &out)).fails(1, "not_found");
    assert!(
        message(&error).contains("has not published the key"),
        "{error}"
    );
    assert!(!Path::new(&out).exists());
}

/// Only the owner can read a cluster's secrets: the server's `owner_only` passes through, and
/// nothing is written.
#[test]
fn owner_only_passes_through() {
    let owner = r#"{"code":"owner_only","error":"Only the owner can do this."}"#;
    let home = home();
    let server = Server::start(vec![
        cluster_route("GET", "", answer(200, &cluster(Some(GUEST)))),
        cluster_route("GET", "secrets/", Reply::json(403, owner)),
        cluster_route("GET", "node-pools/", answer(200, &pools())),
        cluster_route("GET", "operations/", answer(200, &json!([]))),
        cluster_route("POST", "node-pools/3/scale/", Reply::json(403, owner)),
        cluster_route("POST", "nodes/work-1/replace/", Reply::json(403, owner)),
    ]);
    let key = key_path(&home);
    let out = home.path().join("k").to_string_lossy().into_owned();
    let error = Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&[
            "compute",
            "kubernetes",
            "clusters",
            "kubeconfig",
            "42",
            "--cluster-key-file",
            &key,
            "--output-file",
            &out,
        ])
        .fails(1, "owner_only");
    assert_eq!(error["http_status"], 403);
    assert!(!Path::new(&out).exists());
    for args in [
        vec![
            "node-pools",
            "scale",
            "42",
            "3",
            "--count",
            "4",
            "--cluster-key-file",
            &key,
            "--yes",
        ],
        vec![
            "nodes",
            "replace",
            "42",
            "work-1",
            "--cluster-key-file",
            &key,
            "--yes",
        ],
    ] {
        let error = hipc(&home, &server, &args).fails(1, "owner_only");
        assert_eq!(error["side_effects"], "none", "{args:?}");
    }
    for request in server.requests().iter().take(2) {
        assert_eq!(request.header("x-hippius-account"), Some(MEMBER));
    }
}

fn scale_args<'a>(count: &'a str, key: Option<&'a str>, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["node-pools", "scale", "42", "3", "--count", count];
    if let Some(key) = key {
        args.extend(["--cluster-key-file", key]);
    }
    args.extend(extra);
    args
}

fn scale_body(server: &Server) -> Value {
    let requests = server.requests();
    let post: Vec<&Recorded> = requests.iter().filter(|r| r.method == "POST").collect();
    assert_eq!(post.len(), 1, "{:?}", server.calls());
    assert_eq!(
        post[0].path(),
        "/api/kubernetes/clusters/42/node-pools/3/scale/"
    );
    post[0].json()
}

/// A scale up signs one `add_node` per node added, over the pool's `signing_params`, and the
/// count counts adds still queued.
#[test]
fn a_scale_up_signs_one_add_node_per_node_added() {
    let (home, server) = (home(), Server::start(routes(&json!([]))));
    let key = key_path(&home);
    let run = hipc(&home, &server, &scale_args("4", Some(&key), &["--yes"]));
    assert_eq!(run.ok(), json!({ "pool": { "id": 3 }, "operations": [] }));
    // The pools and the operations are read again after the question, then signed.
    assert_eq!(
        server.calls(),
        [
            "GET /api/kubernetes/clusters/42/",
            "GET /api/kubernetes/clusters/42/node-pools/",
            "GET /api/kubernetes/clusters/42/operations/",
            "GET /api/kubernetes/clusters/42/node-pools/",
            "GET /api/kubernetes/clusters/42/operations/",
            "POST /api/kubernetes/clusters/42/node-pools/3/scale/",
        ]
    );
    let body = scale_body(&server);
    assert_eq!(body["count"], 4);
    let operations = body["operations"].as_array().expect("signed operations");
    assert_eq!(operations.len(), 2);
    for op in operations {
        verify(op, PARAMS);
    }
    assert_ne!(operations[0]["request_uuid"], operations[1]["request_uuid"]);
    assert_eq!(body.as_object().unwrap().len(), 2);

    // A queued add counts: two nodes and one add make three, so `--count 4` signs one.
    let queued = json!([{ "id": 5, "kind": "add_node", "state": "queued", "node": null,
                          "params": { "pool": "workers" } }]);
    let server = Server::start(routes(&queued));
    hipc(&home, &server, &scale_args("4", Some(&key), &["--yes"])).ok();
    assert_eq!(
        scale_body(&server)["operations"].as_array().unwrap().len(),
        1
    );
}

/// A scale down signs nothing, and reads the pool again after the question all the same,
/// since the count it sends is applied to the pool as it is then.
#[test]
fn a_scale_down_signs_nothing_and_needs_no_key_file() {
    let (home, server) = (home(), Server::start(routes(&json!([]))));
    hipc(&home, &server, &scale_args("1", None, &["--yes"])).ok();
    assert_eq!(
        server.calls(),
        [
            "GET /api/kubernetes/clusters/42/",
            "GET /api/kubernetes/clusters/42/node-pools/",
            "GET /api/kubernetes/clusters/42/operations/",
            "GET /api/kubernetes/clusters/42/node-pools/",
            "GET /api/kubernetes/clusters/42/operations/",
            "POST /api/kubernetes/clusters/42/node-pools/3/scale/",
        ]
    );
    assert_eq!(scale_body(&server), json!({ "count": 1 }));
    // With a key file given, it is still checked, and still signs nothing.
    let server = Server::start(routes(&json!([])));
    hipc(
        &home,
        &server,
        &scale_args("0", Some(&key_path(&home)), &["--yes"]),
    )
    .ok();
    assert_eq!(scale_body(&server), json!({ "count": 0 }));
    // A pool that signs other params now is still the size the question named, and a scale
    // down signs nothing, so it is sent.
    let mut gpu = pools();
    gpu[1]["signing_params"]["flavor"] = json!("gpu");
    gpu[1]["flavor"] = json!("gpu");
    let mut with_post = changing_routes([pools(), gpu], [json!([]), json!([])]);
    with_post.push(cluster_route(
        "POST",
        "node-pools/3/scale/",
        answer(200, &json!({ "pool": { "id": 3 }, "operations": [] })),
    ));
    let server = Server::start(with_post);
    hipc(&home, &server, &scale_args("1", None, &["--yes"])).ok();
    assert_eq!(scale_body(&server), json!({ "count": 1 }));
}

/// Without `--yes` a scale sends only its reads, and the refusal names the pool, the change,
/// and the cost.
#[test]
fn a_scale_or_replace_without_yes_sends_only_its_reads_and_names_the_cost() {
    let (home, server) = (home(), Server::start(routes(&json!([]))));
    let key = key_path(&home);
    let cases = [
        (
            scale_args("4", Some(&key), &[]),
            "scale the pool `workers` (`3`) of the Kubernetes cluster `prod` (`42`) from 2 to 4 \
             nodes: 2 new `medium` VMs with the image `ubuntu` and Kubernetes \
             `v1.34.3+rke2r1`, each billed to the balance until it is removed, signed with the \
             cluster key file",
        ),
        (
            scale_args("3", Some(&key), &[]),
            "scale the pool `workers` (`3`) of the Kubernetes cluster `prod` (`42`) from 2 to 3 \
             nodes: 1 new `medium` VM with the image `ubuntu` and Kubernetes \
             `v1.34.3+rke2r1`, each billed to the balance until it is removed, signed with the \
             cluster key file",
        ),
        (
            scale_args("1", None, &[]),
            "scale the pool `workers` (`3`) of the Kubernetes cluster `prod` (`42`) from 2 to 1 \
             node: adds still queued are cancelled first, then the newest nodes are drained and \
             their VMs decommissioned; a node with local data blocks until its loss is accepted \
             in the console",
        ),
        (
            vec![
                "nodes",
                "replace",
                "42",
                "work-1",
                "--cluster-key-file",
                &key,
            ],
            "replace the node `work-1` of the pool `workers` (`3`) of the Kubernetes cluster \
             `prod` (`42`): a new `medium` VM with the image `ubuntu` and Kubernetes \
             `v1.34.3+rke2r1` billed to the balance joins first, then `work-1` is drained and \
             its VM decommissioned, and both are billed while they overlap",
        ),
    ];
    for (args, action) in cases {
        let error = hipc(&home, &server, &args).fails(2, "confirmation_required");
        assert_eq!(
            message(&error),
            format!("this would {action}, and `--yes` was not passed"),
            "{args:?}"
        );
    }
    assert!(server.calls().iter().all(|r| r.starts_with("GET ")));
}

#[test]
fn a_scale_that_cannot_be_signed_or_changes_nothing_is_refused_after_the_reads() {
    let (home, server) = (home(), Server::start(routes(&json!([]))));
    let key = key_path(&home);
    let error = hipc(&home, &server, &scale_args("4", None, &["--yes"])).fails(2, "usage");
    assert_eq!(
        message(&error),
        "adding 2 nodes to the pool `workers` (`3`) of the Kubernetes cluster `prod` (`42`) signs \
         each one with the cluster key, and `--cluster-key-file` was not passed"
    );
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .contains("--cluster-key-file")
    );
    let error = hipc(&home, &server, &scale_args("2", Some(&key), &["--yes"])).fails(2, "usage");
    assert_eq!(
        message(&error),
        "the pool `workers` (`3`) of the Kubernetes cluster `prod` (`42`) already has 2 nodes, \
         counting adds still queued"
    );
    // The masters' pool has no `signing_params`, so no node can be signed for it.
    let mut args = scale_args("5", Some(&key), &["--yes"]);
    args[3] = "1";
    let error = hipc(&home, &server, &args).fails(2, "usage");
    assert!(
        message(&error).starts_with(
            "the pool `control-plane` (`1`) of the Kubernetes cluster `prod` (`42`) has no \
             `signing_params`"
        ),
        "{error}"
    );
    args[3] = "9";
    let error = hipc(&home, &server, &args).fails(1, "not_found");
    assert_eq!(
        message(&error),
        "the Kubernetes cluster `prod` (`42`) has no node pool `9`"
    );
    assert_eq!(
        error["hint"],
        "run `hipc compute kubernetes node-pools list 42` to see what exists"
    );
    assert!(server.calls().iter().all(|r| r.starts_with("GET ")));
}

#[test]
fn a_replacement_signs_an_add_node_that_names_the_node_it_replaces() {
    let (home, server) = (home(), Server::start(routes(&json!([]))));
    let key = key_path(&home);
    let run = hipc(
        &home,
        &server,
        &[
            "nodes",
            "replace",
            "42",
            "work-1",
            "--cluster-key-file",
            &key,
            "--yes",
        ],
    );
    assert_eq!(
        run.ok(),
        json!({ "id": 20, "kind": "add_node", "state": "queued" })
    );
    assert_eq!(
        server.calls(),
        [
            "GET /api/kubernetes/clusters/42/",
            "GET /api/kubernetes/clusters/42/node-pools/",
            "GET /api/kubernetes/clusters/42/operations/",
            "GET /api/kubernetes/clusters/42/node-pools/",
            "GET /api/kubernetes/clusters/42/operations/",
            "POST /api/kubernetes/clusters/42/nodes/work-1/replace/",
        ]
    );
    let body = server.requests()[5].json();
    assert_eq!(body.as_object().unwrap().len(), 1);
    let with_replaces = PARAMS.replace(r#""taints""#, r#""replaces":"work-1","taints""#);
    verify(&body["operation"], &with_replaces);
    // A node no pool lists, and a master, are refused after the reads.
    let error = hipc(
        &home,
        &server,
        &[
            "nodes",
            "replace",
            "42",
            "work-9",
            "--cluster-key-file",
            &key,
            "--yes",
        ],
    )
    .fails(1, "not_found");
    assert_eq!(
        message(&error),
        "the Kubernetes cluster `prod` (`42`) has no node `work-9` in its node pools"
    );
    // A node whose removal is done is listed as `removed`, and is not replaced.
    let mut removed = pools();
    removed[1]["nodes"][0]["state"] = json!("removed");
    let gone = Server::start(vec![
        cluster_route("GET", "", answer(200, &cluster(Some(GUEST)))),
        cluster_route("GET", "node-pools/", answer(200, &removed)),
        cluster_route("GET", "operations/", answer(200, &json!([]))),
    ]);
    let error = hipc(&home, &gone, &replace_args(&key)).fails(1, "not_found");
    assert_eq!(
        message(&error),
        "the Kubernetes cluster `prod` (`42`) has no node `work-1` in its node pools"
    );
    let error = hipc(
        &home,
        &server,
        &[
            "nodes",
            "replace",
            "42",
            "m0",
            "--cluster-key-file",
            &key,
            "--yes",
        ],
    )
    .fails(2, "usage");
    assert!(
        message(&error).contains("has no `signing_params`"),
        "{error}"
    );
    assert_eq!(server.requests().len(), 12);
}

#[test]
fn a_dry_run_lists_the_reads_and_a_placeholder_for_the_signatures() {
    let (home, server) = (home(), Server::start(vec![]));
    let key = key_path(&home);
    let url = |suffix: &str| format!("{}/api/kubernetes/clusters/42/{suffix}", server.url());
    let get = |suffix: &str| {
        json!({ "method": "GET", "url": url(suffix), "account_header": null,
                "project_header": null,
                "billing_header": null,
                "body": null })
    };
    let out = home.path().join("k").to_string_lossy().into_owned();
    let mut args = kubeconfig_args(&key, &out);
    args.push("--dry-run");
    let doc = hipc(&home, &server, &args).ok();
    assert_eq!(doc["requests"], json!([get(""), get("secrets/")]));
    // The reads before the question, the two after it, then the write, as a real run sends
    // them (tenet 5).
    let reads = [
        get(""),
        get("node-pools/"),
        get("operations/"),
        get("node-pools/"),
        get("operations/"),
    ];
    let doc = hipc(&home, &server, &scale_args("4", Some(&key), &["--dry-run"])).ok();
    let requests = doc["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 6, "{doc}");
    assert_eq!(&requests[..5], reads);
    assert_eq!(requests[5]["url"], url("node-pools/3/scale/"));
    assert_eq!(requests[5]["body"]["count"], 4);
    assert!(
        requests[5]["body"]["operations"]
            .as_str()
            .unwrap()
            .starts_with("<one add_node signed"),
        "{doc}"
    );
    // A scale down needs no key file, in a dry run as in a real one, and reads again too.
    let doc = hipc(&home, &server, &scale_args("0", None, &["--dry-run"])).ok();
    let requests = doc["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 6, "{doc}");
    assert_eq!(&requests[..5], reads);
    assert_eq!(requests[5]["url"], url("node-pools/3/scale/"));
    assert_eq!(requests[5]["body"]["count"], 0);
    let doc = hipc(
        &home,
        &server,
        &[
            "nodes",
            "replace",
            "42",
            "work-1",
            "--cluster-key-file",
            &key,
            "--dry-run",
        ],
    )
    .ok();
    let requests = doc["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 6, "{doc}");
    assert_eq!(&requests[..5], reads);
    assert_eq!(requests[5]["url"], url("nodes/work-1/replace/"));
    assert!(
        requests[5]["body"]["operation"]
            .as_str()
            .unwrap()
            .starts_with("<an add_node")
    );
    assert!(server.requests().is_empty());
    assert!(!Path::new(&out).exists());
}

#[test]
fn a_bad_count_or_node_name_is_usage_with_zero_requests() {
    let (home, server) = (home(), Server::start(vec![]));
    let key = key_path(&home);
    // clap reads `-1` as a flag, and refuses it as one.
    hipc(&home, &server, &scale_args("-1", Some(&key), &["--yes"])).fails(2, "usage");
    for count in ["1001", "x", "1.5", ""] {
        let error =
            hipc(&home, &server, &scale_args(count, Some(&key), &["--yes"])).fails(2, "usage");
        assert!(
            message(&error).contains("a pool size is a whole number from 0 to 1000"),
            "{count}: {error}"
        );
    }
    hipc(&home, &server, &scale_args("1000", None, &["--dry-run"])).ok();
    for name in ["..", "a/b", "a.b", ""] {
        hipc(
            &home,
            &server,
            &[
                "nodes",
                "replace",
                "42",
                name,
                "--cluster-key-file",
                &key,
                "--yes",
            ],
        )
        .fails(2, "usage");
    }
    // The key file is required where every run signs.
    for args in [
        vec!["nodes", "replace", "42", "work-1", "--yes"],
        vec!["clusters", "kubeconfig", "42", "--output-file", "k"],
        vec!["clusters", "kubeconfig", "42", "--cluster-key-file", &key],
    ] {
        hipc(&home, &server, &args).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

/// A directory that exists but cannot be written is found when the file is created, after the
/// reads: it is still usage, since the caller must name another path, and no file is left.
#[test]
fn a_kubeconfig_directory_that_cannot_be_written_is_usage() {
    if !common::permission_bits_apply() {
        return;
    }
    let (home, server) = (home(), Server::start(routes(&json!([]))));
    let locked = home.path().join("locked");
    std::fs::create_dir(&locked).expect("create the directory");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).expect("chmod");
    let output = locked.join("prod.kubeconfig");
    let out = output.to_string_lossy();
    let error = hipc(&home, &server, &kubeconfig_args(&key_path(&home), &out)).fails(2, "usage");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    assert!(
        message(&error).ends_with("prod.kubeconfig`: permission is denied"),
        "{error}"
    );
    assert_eq!(
        error["hint"],
        "pass `--output-file` a new file in a directory that exists and no other user can write"
    );
    assert!(!output.exists());
    assert_eq!(server.requests().len(), 2);
}

/// A cluster and a pool the API answers without a name are named by their ids, and a pool
/// without an id by its name or by nothing. A pool without a name cannot be the one its
/// `signing_params` name, so it shows its naming through the refusal; a pool with a name
/// and no id, or no flavor, reaches the question.
#[test]
fn a_cluster_and_a_pool_are_named_by_what_the_answer_gives() {
    let mut cluster = cluster(Some(GUEST));
    cluster
        .as_object_mut()
        .expect("an object")
        .shift_remove("name");
    let home = home();
    let key = key_path(&home);
    let replace = |pool: Value| {
        let server = Server::start(vec![
            cluster_route("GET", "", answer(200, &cluster)),
            cluster_route("GET", "node-pools/", answer(200, &json!([pool]))),
            cluster_route("GET", "operations/", answer(200, &json!([]))),
        ]);
        hipc(
            &home,
            &server,
            &[
                "nodes",
                "replace",
                "42",
                "work-1",
                "--cluster-key-file",
                &key,
            ],
        )
    };
    let nodes = json!([{ "name": "work-1" }]);
    let error =
        replace(json!({ "id": 3, "nodes": nodes, "signing_params": {} })).fails(1, "internal");
    assert_eq!(
        message(&error),
        "the pool `3` of the Kubernetes cluster `42` has no name, but its `signing_params` \
         name no name, so no node was signed: the question would name one pool and the \
         signature another"
    );
    // A pool without an id is named by its name; no id is made up, and no flavor either.
    let error = replace(json!({
        "name": "workers",
        "nodes": nodes,
        "signing_params": { "pool": "workers" },
    }))
    .fails(2, "confirmation_required");
    assert_eq!(
        message(&error),
        "this would replace the node `work-1` of the pool `workers` of the Kubernetes cluster \
         `42`: a new VM billed to the balance joins first, then `work-1` is drained and its \
         VM decommissioned, and both are billed while they overlap, and `--yes` was not passed"
    );
    // A pool with neither is named by nothing.
    let error = replace(json!({ "nodes": nodes, "signing_params": { "pool": "workers" } }))
        .fails(1, "internal");
    assert!(
        message(&error).starts_with(
            "the pool of the Kubernetes cluster `42` has no name, but its `signing_params` \
             name `workers`, so no node was signed"
        ),
        "{error}"
    );
}

/// The routes of a scale or a replacement whose pools and operations answer `first` on the
/// read for the question and `then` on the read after it.
fn changing_routes(pools: [Value; 2], operations: [Value; 2]) -> Vec<Route> {
    let [pools_first, pools_then] = pools;
    let [operations_first, operations_then] = operations;
    vec![
        cluster_route("GET", "", answer(200, &cluster(Some(GUEST)))),
        route_each(
            "GET",
            "/api/kubernetes/clusters/42/node-pools/",
            vec![answer(200, &pools_first), answer(200, &pools_then)],
        ),
        route_each(
            "GET",
            "/api/kubernetes/clusters/42/operations/",
            vec![
                answer(200, &operations_first),
                answer(200, &operations_then),
            ],
        ),
    ]
}

fn replace_args(key: &str) -> Vec<&str> {
    vec![
        "nodes",
        "replace",
        "42",
        "work-1",
        "--cluster-key-file",
        key,
        "--yes",
    ]
}

/// The refusal of a pool that changed between the two reads: `conflict`, after the second
/// read, with nothing signed or sent.
fn assert_changed(home: &Home, server: &Server, args: &[&str]) {
    let error = hipc(home, server, args).fails(1, "conflict");
    assert_eq!(
        message(&error),
        "the pool `workers` (`3`) of the Kubernetes cluster `prod` (`42`) changed after hipc \
         read it for the question: its nodes, its operations in flight, or what a node of it \
         signs are not what the question was about, so nothing was signed or sent"
    );
    let hint = error["hint"].as_str().expect("a conflict has a hint");
    assert!(
        hint.starts_with("run `hipc compute kubernetes ")
            && hint.ends_with(" --yes` again: it reads the pool as it is now"),
        "{error}"
    );
    assert_eq!(error["side_effects"], "none");
    assert_eq!(server.requests().len(), 5, "{:?}", server.calls());
    assert!(server.calls().iter().all(|r| r.starts_with("GET ")));
}

/// Another add queued between the read for the question and the read before signing changes
/// how many nodes to add: the scale is refused, so the pool is not grown past `--count`.
#[test]
fn a_scale_whose_count_changed_before_signing_is_refused() {
    let home = home();
    let key = key_path(&home);
    let queued = json!([{ "id": 5, "kind": "add_node", "state": "queued", "node": null,
                          "params": { "pool": "workers" } }]);
    let server = Server::start(changing_routes([pools(), pools()], [json!([]), queued]));
    assert_changed(&home, &server, &scale_args("4", Some(&key), &["--yes"]));
    // A pool that signs other params now, or that is gone, is refused the same way.
    let mut gpu = pools();
    gpu[1]["signing_params"]["flavor"] = json!("gpu");
    gpu[1]["flavor"] = json!("gpu");
    let server = Server::start(changing_routes([pools(), gpu], [json!([]), json!([])]));
    assert_changed(&home, &server, &scale_args("4", Some(&key), &["--yes"]));
    let gone = json!([pools()[0].clone()]);
    let server = Server::start(changing_routes([pools(), gone], [json!([]), json!([])]));
    assert_changed(&home, &server, &scale_args("4", Some(&key), &["--yes"]));
}

/// A scale down sends its count to the pool as it is then, so the pool read again after the
/// question must still be the size the question named: one that grew, by a node or by a
/// queued add, would lose more nodes than the question named, and one that shrank below
/// `--count` would be grown by a POST that signs nothing. Both are refused with no POST.
#[test]
fn a_scale_down_whose_pool_changed_before_the_post_is_refused() {
    let home = home();
    let down = scale_args("1", None, &["--yes"]);
    let mut grew = pools();
    grew[1]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "name": "work-3", "state": "ready" }));
    let server = Server::start(changing_routes([pools(), grew], [json!([]), json!([])]));
    assert_changed(&home, &server, &down);
    let queued = json!([{ "id": 5, "kind": "add_node", "state": "queued", "node": null,
                          "params": { "pool": "workers" } }]);
    let server = Server::start(changing_routes([pools(), pools()], [json!([]), queued]));
    assert_changed(&home, &server, &down);
    // Shrank to nothing, below `--count 1`: the POST would add a node, unsigned.
    let mut empty = pools();
    empty[1]["nodes"] = json!([]);
    let server = Server::start(changing_routes([pools(), empty], [json!([]), json!([])]));
    assert_changed(&home, &server, &down);
    // Shrank to `--count` exactly, or gone: not the pool the question was about either.
    let removing = json!([{ "id": 6, "kind": "remove_node", "state": "running",
                            "node": "work-2" }]);
    let server = Server::start(changing_routes([pools(), pools()], [json!([]), removing]));
    assert_changed(&home, &server, &down);
    let gone = json!([pools()[0].clone()]);
    let server = Server::start(changing_routes([pools(), gone], [json!([]), json!([])]));
    assert_changed(&home, &server, &down);
}

/// A replacement of a node that an operation in flight already removes or replaces is
/// refused before the question; one that starts while the question waits is refused before
/// signing.
#[test]
fn a_replacement_of_a_busy_node_is_refused() {
    let home = home();
    let key = key_path(&home);
    let removing = json!([{ "id": 6, "kind": "remove_node", "state": "running",
                            "node": "work-1" }]);
    let replacing = json!([{ "id": 7, "kind": "add_node", "state": "queued", "node": null,
                             "params": { "pool": "workers", "replaces": "work-1" } }]);
    for (operations, doing) in [(&removing, "removed"), (&replacing, "replaced")] {
        let server = Server::start(routes(operations));
        let error = hipc(&home, &server, &replace_args(&key)).fails(1, "conflict");
        assert_eq!(
            message(&error),
            format!(
                "the node `work-1` of the pool `workers` (`3`) of the Kubernetes cluster `prod` \
                 (`42`) is already being {doing}, so no node was signed"
            )
        );
        assert_eq!(
            error["hint"],
            "run `hipc compute kubernetes operations list 42` to follow that operation"
        );
        assert_eq!(server.requests().len(), 3);
    }
    // Settled operations, and ones on another node, leave it free.
    let settled = json!([
        { "kind": "remove_node", "state": "done", "node": "work-1" },
        { "kind": "add_node", "state": "failed", "params": { "replaces": "work-1" } },
        { "kind": "remove_node", "state": "running", "node": "work-2" },
        { "kind": "add_node", "state": "queued", "params": { "replaces": "work-2" } },
        { "kind": "cancel", "state": "queued", "node": "work-1" }
    ]);
    let server = Server::start(routes(&settled));
    hipc(&home, &server, &replace_args(&key)).ok();
    // A replacement queued while the question waited.
    let server = Server::start(changing_routes([pools(), pools()], [json!([]), replacing]));
    let error = hipc(&home, &server, &replace_args(&key)).fails(1, "conflict");
    assert!(
        message(&error).ends_with("is already being replaced, so no node was signed"),
        "{error}"
    );
    assert_eq!(server.requests().len(), 5);
    // A pool that signs other params now, or no longer holds the node.
    let mut other = pools();
    other[1]["signing_params"]["image"] = json!("debian");
    let server = Server::start(changing_routes([pools(), other], [json!([]), json!([])]));
    assert_changed(&home, &server, &replace_args(&key));
    let mut moved = pools();
    moved[1]["nodes"] = json!([{ "name": "work-2", "state": "ready" }]);
    let server = Server::start(changing_routes([pools(), moved], [json!([]), json!([])]));
    assert_changed(&home, &server, &replace_args(&key));
}

/// The question names the flavor that is signed, and a pool whose `flavor` is not the one its
/// `signing_params` name is refused before the question: another name, `null`, a number, or,
/// for a pool without a flavor, anything signed that is not text.
#[test]
fn a_pool_whose_flavor_is_not_the_signed_one_is_refused() {
    let home = home();
    let key = key_path(&home);
    let workers = "the pool `workers` (`3`) of the Kubernetes cluster `prod` (`42`)";
    let flavored = |signed: Value| {
        let mut pools = pools();
        pools[1]["signing_params"]["flavor"] = signed;
        pools
    };
    let mut bare = pools();
    bare[1].as_object_mut().unwrap().shift_remove("flavor");
    let mut bare_null = bare.clone();
    bare_null[1]["signing_params"]["flavor"] = Value::Null;
    // A pool whose own flavor is not text is refused too, whatever the map signs: the list
    // shows the pool's flavor as it is, and the question could not name it so.
    let mut objected = pools();
    objected[1]["flavor"] = json!({ "cpu": 2 });
    let mismatch = |has: &str, names: &str| {
        format!(
            "{workers} {has}, but its `signing_params` name {names}, so no node was signed: \
             the question would name one flavor and the signature another"
        )
    };
    let servers = [
        (
            reads_only(&flavored(json!("gpu.8x"))),
            mismatch("has the flavor `medium`", "`gpu.8x`"),
        ),
        (
            reads_only(&flavored(Value::Null)),
            mismatch("has the flavor `medium`", "`null`"),
        ),
        (
            reads_only(&flavored(json!(7))),
            mismatch("has the flavor `medium`", "`7`"),
        ),
        (reads_only(&bare_null), mismatch("has no flavor", "`null`")),
        (
            reads_only(&objected),
            format!(
                "{workers} has a flavor that is not text, `{{\"cpu\":2}}`, so no node was \
                 signed: the question would not name the flavor as `hipc compute kubernetes \
                 node-pools list 42` shows it"
            ),
        ),
    ];
    assert_refused_before_the_question(&home, &servers);
    // A pool with no `flavor` of its own is asked about by the signed one.
    let server = reads_only(&bare);
    let error =
        hipc(&home, &server, &scale_args("3", Some(&key), &[])).fails(2, "confirmation_required");
    assert!(message(&error).contains("1 new `medium` VM"), "{error}");
}

/// The reads of a scale or a replacement, with `pools` as the cluster's pools and no write
/// route: a POST would be answered 404, and fail the run.
fn reads_only(pools: &Value) -> Server {
    Server::start(vec![
        cluster_route("GET", "", answer(200, &cluster(Some(GUEST)))),
        cluster_route("GET", "node-pools/", answer(200, pools)),
        cluster_route("GET", "operations/", answer(200, &json!([]))),
    ])
}

/// `signing_params` that would sign what the question does not name are refused before the
/// question, for a scale and for a replacement, with nothing signed or sent: a key hipc does
/// not know, `replaces`, another pool, and labels or taints the pool does not have.
#[test]
fn signing_params_that_would_sign_what_the_question_does_not_name_are_refused() {
    let home = home();
    let workers = "the pool `workers` (`3`) of the Kubernetes cluster `prod` (`42`)";
    let mut unnamed = pools();
    unnamed[1].as_object_mut().unwrap().shift_remove("name");
    unnamed[1]["signing_params"]["pool"] = json!("gpu");
    let mut tainted = pools();
    tainted[1]["taints"] = Value::Null;
    tainted[1]["signing_params"]["taints"] = json!(["spot:NoSchedule"]);
    let mut numbered = pools();
    numbered[1]["name"] = json!(3);
    let cases = [
        (
            "extra",
            json!("x"),
            format!(
                "{workers} has `extra` in its `signing_params`, which hold only `pool`, \
                 `flavor`, `k8s_version`, `image`, `labels`, and `taints`, so no node was \
                 signed: the signature would hold what the question does not name"
            ),
        ),
        (
            "replaces",
            json!("work-2"),
            format!(
                "{workers} has `replaces` in its `signing_params`, which hold only `pool`, \
                 `flavor`, `k8s_version`, `image`, `labels`, and `taints`, so no node was \
                 signed: the signature would hold what the question does not name"
            ),
        ),
        (
            "pool",
            json!("gpu"),
            format!(
                "{workers} has the name `workers`, but its `signing_params` name `gpu`, so no \
                 node was signed: the question would name one pool and the signature another"
            ),
        ),
        (
            "pool",
            json!(3),
            format!(
                "{workers} has the name `workers`, but its `signing_params` name `3`, so no \
                 node was signed: the question would name one pool and the signature another"
            ),
        ),
        (
            "labels",
            json!({ "role": "spy" }),
            format!(
                "{workers} has the labels `{{}}`, but its `signing_params` hold \
                 `{{\"role\":\"spy\"}}`, so no node was signed: the signature would give the \
                 node labels the pool does not have"
            ),
        ),
        (
            "taints",
            json!(["spot:NoSchedule"]),
            format!(
                "{workers} has the taints `[]`, but its `signing_params` hold \
                 `[\"spot:NoSchedule\"]`, so no node was signed: the signature would give the \
                 node taints the pool does not have"
            ),
        ),
    ];
    let mut servers: Vec<(Server, String)> = cases
        .into_iter()
        .map(|(field, value, says)| {
            let mut pools = pools();
            pools[1]["signing_params"][field] = value;
            (reads_only(&pools), says)
        })
        .collect();
    servers.push((
        reads_only(&unnamed),
        "the pool `3` of the Kubernetes cluster `prod` (`42`) has no name, but its \
         `signing_params` name `gpu`, so no node was signed: the question would name one pool \
         and the signature another"
            .to_owned(),
    ));
    // A pool whose `taints` are `null` has none, as one answered without them.
    servers.push((
        reads_only(&tainted),
        format!(
            "{workers} has the taints `[]`, but its `signing_params` hold `[\"spot:NoSchedule\"]`, \
             so no node was signed: the signature would give the node taints the pool does not \
             have"
        ),
    ));
    // A pool whose own name is not text is refused, as the list shows it and the question
    // could not; the message names the pool by its id, since it has no name to show.
    servers.push((
        reads_only(&numbered),
        "the pool `3` of the Kubernetes cluster `prod` (`42`) has a name that is not text, \
         `3`, so no node was signed: the question would not name the pool as `hipc compute \
         kubernetes node-pools list 42` shows it"
            .to_owned(),
    ));
    assert_refused_before_the_question(&home, &servers);
}

/// Each of `servers` refuses a scale and a replacement as `internal` with its message, before
/// the question: the three reads of each run are sent, and no second read or POST.
fn assert_refused_before_the_question(home: &Home, servers: &[(Server, String)]) {
    let key = key_path(home);
    for (server, says) in servers {
        for args in [scale_args("4", Some(&key), &["--yes"]), replace_args(&key)] {
            let error = hipc(home, server, &args).fails(1, "internal");
            assert_eq!(message(&error), says.as_str(), "{args:?}");
            assert_eq!(error["side_effects"], "none");
        }
        assert_eq!(server.requests().len(), 6, "{says}");
        assert!(server.calls().iter().all(|r| r.starts_with("GET ")));
    }
}

/// A key the pool has and its `signing_params` leave out signs a node without it, so it is
/// refused as a changed one is: the pool's name, its flavor, its taints, and its labels,
/// each missing from an otherwise matching map.
#[test]
fn a_key_the_pool_has_and_the_map_leaves_out_is_refused() {
    let home = home();
    let workers = "the pool `workers` (`3`) of the Kubernetes cluster `prod` (`42`)";
    let without = |key: &str| {
        let mut pools = pools();
        pools[1]["signing_params"]
            .as_object_mut()
            .unwrap()
            .shift_remove(key);
        pools
    };
    let mut dedicated = without("taints");
    dedicated[1]["taints"] = json!(["dedicated:NoSchedule"]);
    let mut tiered = without("labels");
    tiered[1]["labels"] = json!({ "tier": "web" });
    let servers = [
        (
            reads_only(&without("pool")),
            format!(
                "{workers} has the name `workers`, but its `signing_params` name no name, so \
                 no node was signed: the question would name one pool and the signature \
                 another"
            ),
        ),
        (
            reads_only(&without("flavor")),
            format!(
                "{workers} has the flavor `medium`, but its `signing_params` name no flavor, \
                 so no node was signed: the question would name one flavor and the signature \
                 another"
            ),
        ),
        (
            reads_only(&dedicated),
            format!(
                "{workers} has the taints `[\"dedicated:NoSchedule\"]`, but its \
                 `signing_params` hold `[]`, so no node was signed: the signature would give \
                 the node taints the pool does not have"
            ),
        ),
        (
            reads_only(&tiered),
            format!(
                "{workers} has the labels `{{\"tier\":\"web\"}}`, but its `signing_params` \
                 hold `{{}}`, so no node was signed: the signature would give the node labels \
                 the pool does not have"
            ),
        ),
    ];
    assert_refused_before_the_question(&home, &servers);
}

/// `signing_params` that match the pool are asked about, and the question names the image,
/// the Kubernetes version, the labels, and the taints only when they are signed; the empty
/// labels and taints of the fixture pool are not named, and ones that are `null` on both
/// sides are signed as `null` and named so. A flavor the question cannot show as text is
/// refused instead.
#[test]
fn the_question_names_what_is_signed_and_refuses_what_it_cannot() {
    let home = home();
    let key = key_path(&home);
    let mut nulled = pools();
    nulled[1]["labels"] = Value::Null;
    nulled[1]["taints"] = Value::Null;
    nulled[1]["signing_params"] =
        json!({ "pool": "workers", "flavor": "medium", "labels": null, "taints": null });
    let mut labelled = pools();
    labelled[1]["labels"] = json!({ "tier": "web" });
    labelled[1]["taints"] = json!(["spot:NoSchedule"]);
    labelled[1]["signing_params"] = json!({
        "pool": "workers",
        "flavor": "medium",
        "labels": { "tier": "web" },
        "taints": ["spot:NoSchedule"],
    });
    let mut imaged = pools();
    imaged[1]["signing_params"] =
        json!({ "pool": "workers", "flavor": "medium", "image": "ubuntu" });
    let mut versioned = pools();
    versioned[1]["signing_params"] =
        json!({ "pool": "workers", "flavor": "medium", "k8s_version": 7 });
    // A pool without a flavor of its own gets no flavor check, so a flavor signed as a number
    // or an object is refused rather than shown, or signed unnamed.
    let mut numbered = pools();
    numbered[1].as_object_mut().unwrap().shift_remove("flavor");
    numbered[1]["signing_params"] = json!({ "pool": "workers", "flavor": 7 });
    let mut objected = pools();
    objected[1].as_object_mut().unwrap().shift_remove("flavor");
    objected[1]["signing_params"] =
        json!({ "pool": "workers", "flavor": { "cpu": 2 }, "image": "ubuntu" });
    let refused = [(numbered, "`7`"), (objected, "`{\"cpu\":2}`")].map(|(pools, names)| {
        (
            reads_only(&pools),
            format!(
                "the pool `workers` (`3`) of the Kubernetes cluster `prod` (`42`) has no \
                 flavor, but its `signing_params` name {names}, so no node was signed: the \
                 question would name one flavor and the signature another"
            ),
        )
    });
    assert_refused_before_the_question(&home, &refused);
    for (pools, vms) in [
        (
            nulled,
            "2 new `medium` VMs with the labels `null` and the taints `null`, each",
        ),
        (
            labelled,
            "2 new `medium` VMs with the labels `{\"tier\":\"web\"}` and the taints \
             `[\"spot:NoSchedule\"]`, each",
        ),
        (imaged, "2 new `medium` VMs with the image `ubuntu`, each"),
        (versioned, "2 new `medium` VMs with Kubernetes `7`, each"),
    ] {
        let server = reads_only(&pools);
        let error = hipc(&home, &server, &scale_args("4", Some(&key), &[]))
            .fails(2, "confirmation_required");
        assert!(message(&error).contains(vms), "{error}");
        assert_eq!(server.requests().len(), 3);
    }
}

/// `seconds` since the epoch as an IMF-fixdate, by Howard Hinnant's `civil_from_days`;
/// 1970-01-01 was a Thursday. The scripted server sends no `Date` of its own, and a fixed
/// one would be far from the clock of the machine that runs the test.
fn imf_fixdate(seconds: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = seconds / 86_400;
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    let clock = seconds % 86_400;
    let index = |n: u64| usize::try_from(n).expect("a weekday or a month fits");
    format!(
        "{}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
        DAYS[index(days % 7)],
        MONTHS[index(month - 1)],
        clock / 3_600,
        clock / 60 % 60,
        clock % 60
    )
}

/// The signatures are dated by the server's clock: the `Date` of the read before signing,
/// when it is within five minutes of the local clock. One further off is refused, with
/// nothing signed or sent, rather than replaced by the local clock.
#[test]
fn a_signature_is_dated_by_the_server_clock_when_it_agrees_with_the_local_one() {
    let home = home();
    let key = key_path(&home);
    assert_eq!(imf_fixdate(784_111_777), "Sun, 06 Nov 1994 08:49:37 GMT");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs();
    let dated = |date: &str| answer(200, &json!([])).header("Date", date);
    let routes = |then: &str| {
        vec![
            cluster_route(
                "GET",
                "",
                answer(200, &cluster(Some(GUEST))).header("Date", "Thu, 01 Jan 1970 00:00:09 GMT"),
            ),
            cluster_route("GET", "node-pools/", answer(200, &pools())),
            route_each(
                "GET",
                "/api/kubernetes/clusters/42/operations/",
                vec![dated(&imf_fixdate(now - 200)), dated(then)],
            ),
            cluster_route(
                "POST",
                "node-pools/3/scale/",
                answer(200, &json!({ "pool": { "id": 3 } })),
            ),
        ]
    };
    // The read for the question and the read before signing answer at different times; the
    // signature takes the second, and neither the cluster read's nor the local clock.
    let server = Server::start(routes(&imf_fixdate(now - 100)));
    hipc(&home, &server, &scale_args("3", Some(&key), &["--yes"])).ok();
    let op = &scale_body(&server)["operations"][0];
    assert_eq!(op["issued_at"], now - 100);
    verify(op, PARAMS);
    for far in [now + 3600, now - 360, 951_825_600] {
        let date = imf_fixdate(far);
        let server = Server::start(routes(&date));
        let error =
            hipc(&home, &server, &scale_args("3", Some(&key), &["--yes"])).fails(1, "internal");
        assert_eq!(
            message(&error),
            format!(
                "cannot sign a node operation: the API's `Date` `{date}` and the local clock \
                 disagree by more than 5 minutes, so no signature is dated by either"
            )
        );
        // The hint keeps both halves: the clock to check, and the report an `internal` error
        // asks for, since a server that answers far off is a bug to report.
        assert_eq!(
            error["hint"],
            "check the system clock, then run this again with `--debug` and report what it \
             prints at `https://github.com/thenervelab/hippius-cli/issues`"
        );
        assert_eq!(error["side_effects"], "none");
        assert_eq!(server.requests().len(), 5);
        assert!(server.calls().iter().all(|r| r.starts_with("GET ")));
    }
}

/// A kubeconfig in a directory another user can write could be swapped for one that names
/// their server, so the directory is refused at parse time, before any read, and again on the
/// open directory when the file is created: a directory opened to others during the reads is
/// refused then, and no file is left.
#[test]
fn a_kubeconfig_directory_others_can_write_is_usage() {
    let home = home();
    let key = key_path(&home);
    let open = home.path().join("open");
    std::fs::create_dir(&open).expect("create the directory");
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).expect("chmod");
    let out = open.join("prod.kubeconfig");
    let server = Server::start(routes(&json!([])));
    let error = hipc(
        &home,
        &server,
        &kubeconfig_args(&key, &out.to_string_lossy()),
    )
    .fails(2, "usage");
    assert!(
        message(&error).contains("can be written by other users"),
        "{error}"
    );
    assert!(server.requests().is_empty());

    let later = home.path().join("later");
    std::fs::create_dir(&later).expect("create the directory");
    std::fs::set_permissions(&later, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    let opened = later.clone();
    let mut routes = routes(&json!([]));
    routes[0] = cluster_route(
        "GET",
        "",
        answer(200, &cluster(Some(GUEST))).before(move || {
            drop(std::fs::set_permissions(
                &opened,
                std::fs::Permissions::from_mode(0o777),
            ));
        }),
    );
    let server = Server::start(routes);
    let out = later.join("prod.kubeconfig");
    let error = hipc(
        &home,
        &server,
        &kubeconfig_args(&key, &out.to_string_lossy()),
    )
    .fails(2, "usage");
    assert!(
        message(&error).ends_with("prod.kubeconfig`: its directory can be written by other users"),
        "{error}"
    );
    assert!(!server.requests().is_empty());
    assert!(!out.exists());
}

/// A scale or a replacement whose signed `POST` is answered with a 5xx, or with a 2xx hipc
/// cannot read, may have queued the nodes it signed, which are billed: each is `partial`, not
/// retryable, with the hint to check before running it again, so a retry does not add them
/// twice.
#[test]
fn a_signed_post_answered_5xx_or_unreadable_is_partial_with_the_check_first_hint() {
    let home = home();
    let key = key_path(&home);
    let scale = scale_args("4", Some(&key), &["--yes"]);
    let replace = replace_args(&key);
    // The two POST routes are the last two of `routes`.
    for (args, post, suffix) in [
        (&scale, 4, "node-pools/3/scale/"),
        (&replace, 5, "nodes/work-1/replace/"),
    ] {
        for (reply, code) in [
            (Reply::json(500, r#"{"detail":"boom"}"#), "internal"),
            (
                Reply::json(502, r#"{"detail":"bad gateway"}"#),
                "unavailable",
            ),
            (Reply::json(503, r#"{"detail":"down"}"#), "unavailable"),
            (
                Reply::bytes(200, "text/html", b"<html>ok</html>"),
                "internal",
            ),
            (
                Reply::bytes(202, "application/json", b"{\"id\":"),
                "internal",
            ),
        ] {
            let mut routes = routes(&json!([]));
            routes[post] = cluster_route("POST", suffix, reply);
            let server = Server::start(routes);
            let error = hipc(&home, &server, args).fails(1, code);
            assert_eq!(error["side_effects"], "partial", "{suffix}: {error}");
            assert_eq!(error["retryable"], false, "{suffix}: {error}");
            assert_eq!(
                error["hint"], "check whether the change was made before running the command again",
                "{suffix}: {error}"
            );
            let posts = server
                .calls()
                .into_iter()
                .filter(|r| r.starts_with("POST "))
                .count();
            assert_eq!(posts, 1, "{suffix}: one signed POST, not retried");
        }
    }
}
