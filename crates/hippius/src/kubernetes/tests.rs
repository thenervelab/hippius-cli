//! The cluster key against the console's known-answer vectors, which the console made with the
//! backend's own code (`PyNaCl` `SealedBox` and `SigningKey`, and Python's `json.dumps`). If a
//! vector stops passing, a real cluster refuses what `hipc` signs or hands back a kubeconfig
//! `hipc` cannot open: fix the code, never the vector.
//!
//! The keys of the vectors:
//!
//! - box secret: the bytes 1 to 32;
//! - client signing seed: the bytes 11 to 42;
//! - guest signing seed: the bytes 101 to 132, and another guest's: 201 to 232;
//! - cluster id: 42.

use std::fs;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::sync::atomic::{AtomicUsize, Ordering};

use crypto_box::aead::OsRng;

use super::*;

const BOX_PUBLIC: &str = "B6N8vBQgk8i3VdwbEOhstCY3StFqqFPtC9/AsrhtHHw=";
const SIGN_PUBLIC: &str = "VCgh6VKd0a0kO/3/XGQKZRlLSBaYemUN6GHXzp5BemM=";
const GUEST: &str = "2inpWwLgD/oVZFd1+x0roiKhlDOV7qBrlOLAV7e+adA=";
const OTHER_GUEST: &str = "p/z33O1VMdWsOFzHvaGk630A1iSKf4+9jbvd9zoh0qA=";

const KUBECONFIG: &str = "apiVersion: v1\nkind: Config\nclusters:\n- name: m0\n  cluster:\n    \
                          certificate-authority-data: Q0E=\n    server: \
                          https://203.0.113.10:6443\ncurrent-context: m0\n";
const KUBECONFIG_CIPHERTEXT: &str = "bTznQo66nmdAIkFNAD1I39+e5osoMThoZYz3OTlZby2ZZdEy8DN676jADwsFO\
    WK1EZNEDO5MzlSsGkkPw3qgd5bOEol2oLoSL7Bs3E/7KsoiIQev8ITgt42x7+DCBU0cM7Rxy6BN3gasC8an8vwYo9Eh\
    xiaPhpw0GlFQlCeF6ElDgKcdSwju1QYlNy2xUP0MDhJ3S4sQASFwHDIscP3y1J8B+C4l4etEqIRRaLisJNMxKItf0s\
    E4/Y1kbHmQxJVPxcTooKnkPmLB9es=";
const KUBECONFIG_SIGNATURE: &str =
    "MfGE71Br/NFmaEal6U93MGk2tDY9W+6pv6kLO4cPwdOlYGBC+3b9acFYMF/HTfTwXgrJkeaaAYz5+wohCZXyBA==";

const SERVER_TOKEN: &str = "K10abc::server:0123456789abcdef";
const SERVER_TOKEN_CIPHERTEXT: &str = "RFvgRwBVAtmeYQSczaR7d5cVt96cdbsz1bCUFgPazisFdHMXY97XccZkwsC\
    TPg14oUh7Az02x5pHKPhJB60xDMrHE1PMhRtEsc5ElwYLZQ==";
const SERVER_TOKEN_SIGNATURE: &str =
    "saz4DH/wPIahTY755dRz7L+04N2eA+xfbzlrNzY0dOsoFrWwjKsbpAJ+Lb3wG3C4C1UQ1qThD1cfncfaW9QlBA==";

/// The bytes `from..to`.
fn range(from: u8, to: u8) -> [u8; 32] {
    let bytes: Vec<u8> = (from..to).collect();
    bytes
        .try_into()
        .expect("each range in these tests is 32 bytes")
}

fn b64(bytes: &[u8]) -> String {
    Base64::encode_string(bytes)
}

/// The key file the console saves, with each key given in base64.
fn key_file_with(
    sign_secret: &str,
    sign_public: &str,
    box_secret: &str,
    box_public: &str,
) -> String {
    json!({
        "version": 1,
        "kind": "hippius-k8s-cluster-key",
        "cluster_name": "prod",
        "created_at": "2026-10-01T00:00:00.000Z",
        "sign_secret_key": sign_secret,
        "sign_public_key": sign_public,
        "box_secret_key": box_secret,
        "box_public_key": box_public,
    })
    .to_string()
}

/// The key file of the vectors.
fn vector_key_file() -> String {
    key_file_with(
        &b64(&range(11, 43)),
        SIGN_PUBLIC,
        &b64(&range(1, 33)),
        BOX_PUBLIC,
    )
}

/// A private directory, removed first if a crashed run left it.
fn temp_dir(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("hipc-kubernetes-{name}-{}-{n}", std::process::id()));
    drop(fs::remove_dir_all(&dir));
    fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

/// Writes `text` as a key file with `mode` in a new private directory, and loads it.
fn load_text(name: &str, text: &[u8], mode: u32) -> Result<ClusterKey> {
    let dir = temp_dir(name);
    let path = dir.join("hippius-cluster-prod.key");
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    let loaded = ClusterKey::load(&path, "--cluster-key-file");
    fs::remove_dir_all(&dir).unwrap();
    loaded
}

fn vector_key() -> ClusterKey {
    load_text("vector", vector_key_file().as_bytes(), 0o600).unwrap()
}

/// The cluster 42 as the API returns it, created with the vector key.
fn cluster(guest: Option<&str>) -> Value {
    json!({
        "id": 42,
        "name": "prod",
        "client_sign_pubkey": SIGN_PUBLIC,
        "client_box_pubkey": BOX_PUBLIC,
        "guest_sign_pubkey": guest,
    })
}

/// The secrets answer with `drops`, signed by `guest`.
fn secrets(guest: &str, drops: &Value) -> Value {
    json!({ "guest_sign_pubkey": guest, "drops": drops })
}

fn drop_of(kind: &str, ciphertext: &str, signature: &str, created_at: &str) -> Value {
    json!({
        "id": 1,
        "kind": kind,
        "operation": null,
        "ciphertext": ciphertext,
        "signature": signature,
        "created_at": created_at,
    })
}

fn kubeconfig_drop() -> Value {
    drop_of(
        "kubeconfig",
        KUBECONFIG_CIPHERTEXT,
        KUBECONFIG_SIGNATURE,
        "2026-10-01T00:00:00Z",
    )
}

/// The vector key, matched to the cluster 42 with the vector guest key.
fn open(key: &ClusterKey, secrets: &Value) -> Result<Zeroizing<String>> {
    key.matched(42, &cluster(Some(GUEST)))?
        .open_kubeconfig(secrets)
}

/// A drop of the cluster 42 that the vector guest signed: `plain` sealed to the box key whose
/// secret is `to`, in base64, and its signature.
fn guest_drop(plain: &[u8], to: [u8; 32], kind: &str) -> (String, String) {
    let sealed = crypto_box::SecretKey::from(to)
        .public_key()
        .seal(&mut OsRng, plain)
        .unwrap();
    let ciphertext = b64(&sealed);
    let guest = SigningKey::from_bytes(&range(101, 133));
    let signature = guest.sign(drop_message(42, kind, &ciphertext).as_bytes());
    (ciphertext, b64(&signature.to_bytes()))
}

/// Flips the lowest bit of the byte at `index` of what `text` decodes to.
fn flip_bit(text: &str, index: usize) -> String {
    let mut bytes = Base64::decode_vec(text).unwrap();
    bytes[index] ^= 1;
    b64(&bytes)
}

#[test]
fn the_keys_derive_the_public_keys_pynacl_derives() {
    let key = vector_key();
    assert_eq!(key.sign_public, SIGN_PUBLIC);
    assert_eq!(key.box_public, BOX_PUBLIC);
    let guest = SigningKey::from_bytes(&range(101, 133));
    assert_eq!(b64(guest.verifying_key().as_bytes()), GUEST);
    let other = SigningKey::from_bytes(&range(201, 233));
    assert_eq!(b64(other.verifying_key().as_bytes()), OTHER_GUEST);
}

#[test]
fn the_drop_message_hashes_the_base64_text() {
    // sha256("YWJj"): of the base64 text, not of the bytes it decodes to.
    assert_eq!(
        drop_message(42, "kubeconfig", "YWJj"),
        "hippius-k8s-drop:v1:42:kubeconfig:\
         35d95694d3f160215db293c7899daa5907837838fb4b8119ed713e32446c1266"
    );
}

#[test]
fn both_vector_drops_verify_and_are_bound_to_the_cluster_and_the_kind() {
    for (kind, ciphertext, signature) in [
        ("kubeconfig", KUBECONFIG_CIPHERTEXT, KUBECONFIG_SIGNATURE),
        (
            "server_token",
            SERVER_TOKEN_CIPHERTEXT,
            SERVER_TOKEN_SIGNATURE,
        ),
    ] {
        assert!(
            verify_drop(GUEST, 42, kind, ciphertext, signature),
            "{kind}"
        );
        assert!(
            !verify_drop(GUEST, 43, kind, ciphertext, signature),
            "{kind}"
        );
        assert!(
            !verify_drop(OTHER_GUEST, 42, kind, ciphertext, signature),
            "{kind}"
        );
    }
    assert!(!verify_drop(
        GUEST,
        42,
        "server_token",
        KUBECONFIG_CIPHERTEXT,
        KUBECONFIG_SIGNATURE
    ));
    // A guest key or a signature that is not base64 of the right length, and a guest key
    // that is no curve point: the first `y` whose encoding does not decompress.
    let not_a_point = (0_u8..=255)
        .map(|y| {
            let mut encoded = [0_u8; 32];
            encoded[0] = y;
            encoded
        })
        .find(|encoded| VerifyingKey::from_bytes(encoded).is_err())
        .expect("about half of all encodings are not a point");
    let not_a_point = b64(&not_a_point);
    for (guest, signature) in [
        ("AAAA", KUBECONFIG_SIGNATURE),
        (GUEST, "AAAA"),
        ("not base64!", KUBECONFIG_SIGNATURE),
        (not_a_point.as_str(), KUBECONFIG_SIGNATURE),
    ] {
        assert!(
            !verify_drop(guest, 42, "kubeconfig", KUBECONFIG_CIPHERTEXT, signature),
            "{guest} {signature}"
        );
    }
}

#[test]
fn the_vector_kubeconfig_and_server_token_open() {
    let key = vector_key();
    let opened = open(&key, &secrets(GUEST, &json!([kubeconfig_drop()]))).unwrap();
    assert_eq!(opened.as_str(), KUBECONFIG);
    let sealed = Base64::decode_vec(SERVER_TOKEN_CIPHERTEXT).unwrap();
    assert_eq!(
        box_key(&key.box_secret).unseal(&sealed).unwrap(),
        SERVER_TOKEN.as_bytes()
    );
}

#[test]
fn a_box_sealed_here_opens_and_the_newest_launch_drop_is_the_one_opened() {
    let key = vector_key();
    let (ciphertext, signature) = guest_drop(b"kind: Config\n", range(1, 33), "kubeconfig");
    let mut later = drop_of(
        "kubeconfig",
        &ciphertext,
        &signature,
        "2026-10-02T00:00:00Z",
    );
    later["id"] = json!(2);
    // An operation's drop, newer still, and a server token are not the launch kubeconfig.
    let mut operation = kubeconfig_drop();
    operation["operation"] = json!(9);
    operation["created_at"] = json!("2026-10-03T00:00:00Z");
    let token = drop_of(
        "server_token",
        SERVER_TOKEN_CIPHERTEXT,
        SERVER_TOKEN_SIGNATURE,
        "2026-10-04T00:00:00Z",
    );
    let drops = json!([kubeconfig_drop(), later, operation, token]);
    let opened = open(&key, &secrets(GUEST, &drops)).unwrap();
    assert_eq!(opened.as_str(), "kind: Config\n");
    // A drop without `operation` is a launch drop too.
    let mut bare = kubeconfig_drop();
    bare.as_object_mut().unwrap().shift_remove("operation");
    let opened = open(&key, &secrets(GUEST, &json!([bare]))).unwrap();
    assert_eq!(opened.as_str(), KUBECONFIG);
}

/// `created_at` is read as a time: a fraction makes a drop later, though `...:00.5Z` sorts
/// before `...:00Z` as text, and a drop without a ciphertext or a signature is passed over for
/// the newest one that has both.
#[test]
fn the_newest_launch_drop_is_found_by_its_time_among_those_that_can_be_opened() {
    let key = vector_key();
    let (ciphertext, signature) = guest_drop(b"kind: Config\n", range(1, 33), "kubeconfig");
    let newer = drop_of(
        "kubeconfig",
        &ciphertext,
        &signature,
        "2026-10-01T00:00:00.5Z",
    );
    let older = drop_of(
        "kubeconfig",
        KUBECONFIG_CIPHERTEXT,
        KUBECONFIG_SIGNATURE,
        "2026-10-01T00:00:00Z",
    );
    let opened = open(&key, &secrets(GUEST, &json!([newer, older]))).unwrap();
    assert_eq!(opened.as_str(), "kind: Config\n");
    let opened = open(&key, &secrets(GUEST, &json!([older, newer]))).unwrap();
    assert_eq!(opened.as_str(), "kind: Config\n");
    // An offset is read too: 02:00 at +03:00 is before 00:00 UTC.
    let mut east = newer.clone();
    east["created_at"] = json!("2026-10-01T02:00:00+03:00");
    let opened = open(&key, &secrets(GUEST, &json!([older, east]))).unwrap();
    assert_eq!(opened.as_str(), KUBECONFIG);
    // A time that is not one is older than any that is.
    let mut garbled = newer;
    garbled["created_at"] = json!("2026-10-99T00:00:00Z");
    let opened = open(&key, &secrets(GUEST, &json!([garbled, older]))).unwrap();
    assert_eq!(opened.as_str(), KUBECONFIG);
    for field in ["ciphertext", "signature"] {
        let mut unusable = drop_of("kubeconfig", "", "", "2026-10-09T00:00:00Z");
        unusable.as_object_mut().unwrap().shift_remove(field);
        unusable[if field == "ciphertext" {
            "signature"
        } else {
            "ciphertext"
        }] = json!("x");
        let opened = open(&key, &secrets(GUEST, &json!([unusable, older]))).unwrap();
        assert_eq!(opened.as_str(), KUBECONFIG, "{field}");
        unusable[field] = json!(null);
        let opened = open(&key, &secrets(GUEST, &json!([older, unusable]))).unwrap();
        assert_eq!(opened.as_str(), KUBECONFIG, "{field}");
    }
}

#[test]
fn an_rfc3339_time_is_a_real_date_with_a_real_offset() {
    for (text, seconds, nanos) in [
        ("2024-02-29T00:00:00Z", 1_709_164_800, 0),
        ("2000-02-29T00:00:00Z", 951_782_400, 0),
        ("2026-12-31T23:59:59Z", 1_798_761_599, 0),
        ("2026-11-03T10:00:00.5Z", 1_793_700_000, 500_000_000),
        (
            "2026-11-03T10:00:00.123456789123Z",
            1_793_700_000,
            123_456_789,
        ),
        ("2026-11-03T13:59:00+03:59", 1_793_700_000, 0),
        ("2026-11-03T10:00:00-00:00", 1_793_700_000, 0),
    ] {
        assert_eq!(rfc3339(text), Some((seconds, nanos)), "{text}");
    }
    for bad in [
        "2026-02-29T00:00:00Z",
        "2100-02-29T00:00:00Z",
        "2026-02-31T00:00:00Z",
        "2026-04-31T00:00:00Z",
        "2026-11-03T10:00:00+24:00",
        "2026-11-03T10:00:00+99:99",
        "2026-11-03T10:00:00+02:60",
        "2026-11-03T10:00:00+2:00",
        "2026-11-03T10:00:00+002:00",
        "2026-1-03T10:00:00Z",
        "2026-11-3T10:00:00Z",
        "26-11-03T10:00:00Z",
        "2026-11-03T1:00:00Z",
        "2026-11-03T10:00:00:00Z",
        "2026-11-03-01T10:00:00Z",
        "2026-11-03T10:00:00Z02:00",
    ] {
        assert_eq!(rfc3339(bad), None, "{bad}");
    }
}

#[test]
fn a_drop_that_does_not_verify_is_refused_before_it_is_opened() {
    let key = vector_key();
    let mut tampered = kubeconfig_drop();
    tampered["ciphertext"] = json!(flip_bit(KUBECONFIG_CIPHERTEXT, 40));
    let mut forged = kubeconfig_drop();
    forged["signature"] = json!(flip_bit(KUBECONFIG_SIGNATURE, 3));
    for drop in [tampered, forged] {
        let error = open(&key, &secrets(GUEST, &json!([drop]))).unwrap_err();
        assert_eq!(error.code(), Code::Internal);
        assert_eq!(
            error.message(),
            "the kubeconfig of the Kubernetes cluster `prod` (`42`) is not signed by the \
             cluster's guest key, and it was not decrypted, so it was refused"
        );
    }
    // Checked against another guest key, the vector drop does not verify either.
    let error = key
        .matched(42, &cluster(Some(OTHER_GUEST)))
        .unwrap()
        .open_kubeconfig(&secrets(OTHER_GUEST, &json!([kubeconfig_drop()])))
        .unwrap_err();
    assert!(
        error.message().contains("is not signed"),
        "{}",
        error.message()
    );
}

#[test]
fn a_signed_drop_that_will_not_open_is_refused() {
    let key = vector_key();
    let cases = [
        // Sealed to another key.
        (
            guest_drop(KUBECONFIG.as_bytes(), range(50, 82), "kubeconfig"),
            "does not open",
        ),
        // Sealed to this key, but not text.
        (
            guest_drop(&[0xff, 0xfe], range(1, 33), "kubeconfig"),
            "is not UTF-8 text",
        ),
    ];
    for ((ciphertext, signature), why) in cases {
        let drops = json!([drop_of("kubeconfig", &ciphertext, &signature, "z")]);
        let error = open(&key, &secrets(GUEST, &drops)).unwrap_err();
        assert_eq!(error.code(), Code::Internal);
        assert!(error.message().contains(why), "{}", error.message());
    }
    // Signed, but not base64.
    let ciphertext = "not base64!";
    let guest = SigningKey::from_bytes(&range(101, 133));
    let signature = b64(&guest
        .sign(drop_message(42, "kubeconfig", ciphertext).as_bytes())
        .to_bytes());
    let drops = json!([drop_of("kubeconfig", ciphertext, &signature, "z")]);
    let error = open(&key, &secrets(GUEST, &drops)).unwrap_err();
    assert_eq!(
        error.message(),
        "the kubeconfig of the Kubernetes cluster `prod` (`42`) is not base64, so it was \
         refused"
    );
}

#[test]
fn a_cluster_without_its_guest_key_or_its_drop_is_not_found_yet() {
    let key = vector_key();
    let error = key
        .matched(42, &cluster(None))
        .unwrap()
        .open_kubeconfig(&secrets(GUEST, &json!([kubeconfig_drop()])))
        .unwrap_err();
    assert_eq!(error.code(), Code::NotFound);
    assert_eq!(
        error.message(),
        "the Kubernetes cluster `prod` (`42`) has not published the key that signs its \
         kubeconfig yet"
    );
    assert!(error.hint().unwrap().contains("5 to 10 minutes"));
    let mut operation = kubeconfig_drop();
    operation["operation"] = json!(3);
    let no_kubeconfig = [
        json!({ "guest_sign_pubkey": GUEST }),
        secrets(GUEST, &json!([])),
        secrets(GUEST, &json!({})),
        secrets(GUEST, &json!([operation])),
        secrets(
            GUEST,
            &json!([{ "kind": "kubeconfig", "ciphertext": "AAAA" }]),
        ),
        secrets(
            GUEST,
            &json!([{ "kind": "kubeconfig", "signature": "AAAA" }]),
        ),
    ];
    for answer in no_kubeconfig {
        let error = open(&key, &answer).unwrap_err();
        assert_eq!(error.code(), Code::NotFound, "{answer}");
        assert_eq!(
            error.message(),
            "the Kubernetes cluster `prod` (`42`) has not handed over its kubeconfig yet"
        );
    }
}

#[test]
fn a_guest_key_that_changed_between_the_reads_is_refused() {
    let key = vector_key();
    for answer in [
        secrets(OTHER_GUEST, &json!([kubeconfig_drop()])),
        json!({ "drops": [kubeconfig_drop()] }),
    ] {
        let error = open(&key, &answer).unwrap_err();
        assert_eq!(error.code(), Code::Internal);
        assert_eq!(
            error.message(),
            "the key that signs the secrets of the Kubernetes cluster `prod` (`42`) changed \
             between two reads, so nothing was decrypted"
        );
    }
}

#[test]
fn a_key_file_is_matched_to_its_cluster_by_both_public_keys() {
    let key = vector_key();
    let other = b64(&[7; 32]);
    for field in ["client_sign_pubkey", "client_box_pubkey"] {
        let mut theirs = cluster(Some(GUEST));
        theirs[field] = json!(other);
        let error = key.matched(42, &theirs).unwrap_err();
        assert_eq!(error.code(), Code::Usage, "{field}");
        assert!(
            error.message().ends_with(
                "is not the key of the Kubernetes cluster `prod` (`42`): its public keys are \
                 not the ones the cluster was created with"
            ),
            "{}",
            error.message()
        );
        assert_eq!(
            error.hint(),
            Some(
                "pass `--cluster-key-file` the `hippius-cluster-<name>.key` file the console \
                 saved when it created the cluster"
            )
        );
        theirs.as_object_mut().unwrap().shift_remove(field);
        let error = key.matched(42, &theirs).unwrap_err();
        assert_eq!(error.code(), Code::Internal, "{field}");
        assert_eq!(
            error.message(),
            format!(
                "cannot check the cluster key file against the Kubernetes cluster `prod` \
                 (`42`): the API answered without its `{field}`"
            )
        );
    }
    let unnamed = json!({ "client_sign_pubkey": other, "client_box_pubkey": BOX_PUBLIC });
    let error = key.matched(42, &unnamed).unwrap_err();
    assert!(
        error.message().contains("the Kubernetes cluster `42`:"),
        "{}",
        error.message()
    );
}

/// The map of a `json!` object literal.
fn object(literal: Value) -> Map<String, Value> {
    serde_json::from_value(literal).expect("the literal is a JSON object")
}

/// The `add_node` of the console's vector, its params out of order at both depths.
fn add_node_params() -> Map<String, Value> {
    object(json!({
        "pool": "gpu",
        "flavor": "large",
        "k8s_version": "v1.34.3+rke2r1",
        "image": "ubuntu",
        "labels": { "zone": "b", "app": "web", "team.example.com/owner": "ops" },
        "taints": ["gpu=true:NoSchedule", "dedicated:PreferNoSchedule"],
    }))
}

/// The canonical bytes of the `add_node` envelope of `params`, as a test builds them.
fn envelope_bytes(cluster: u64, uuid: &str, params: &Map<String, Value>, iat: u64) -> String {
    let envelope = json!({
        "v": 1, "dom": "hippius-k8s-op", "cluster": cluster.to_string(), "req": uuid,
        "kind": "add_node", "params": params, "iat": iat, "exp": iat + 3000, "kv": 1,
    });
    canonical(&envelope).unwrap()
}

/// Signs the `add_node` of `params` for the cluster `id` with the vector key, at `iat` with
/// `uuid`.
fn sign_vector(id: u64, params: &Map<String, Value>, uuid: &str, iat: u64) -> Value {
    let key = vector_key();
    let mut cluster = cluster(Some(GUEST));
    cluster["id"] = json!(id);
    key.matched(id, &cluster)
        .unwrap()
        .add_node_at(params, uuid, iat)
        .unwrap()
}

#[test]
fn an_add_node_is_signed_exactly_as_pynacl_signs_it() {
    let uuid = "3f2b8c1e-9d4a-4b6f-8e21-5c7a9b0d1e2f";
    let params = add_node_params();
    let bytes = envelope_bytes(42, uuid, &params, 1_790_000_000);
    assert_eq!(
        bytes,
        r#"{"cluster":"42","dom":"hippius-k8s-op","exp":1790003000,"iat":1790000000,"kind":"add_node","kv":1,"params":{"flavor":"large","image":"ubuntu","k8s_version":"v1.34.3+rke2r1","labels":{"app":"web","team.example.com/owner":"ops","zone":"b"},"pool":"gpu","taints":["gpu=true:NoSchedule","dedicated:PreferNoSchedule"]},"req":"3f2b8c1e-9d4a-4b6f-8e21-5c7a9b0d1e2f","v":1}"#
    );
    assert_eq!(
        b64(bytes.as_bytes()),
        "eyJjbHVzdGVyIjoiNDIiLCJkb20iOiJoaXBwaXVzLWs4cy1vcCIsImV4cCI6MTc5MDAwMzAwMCwiaWF0IjoxNz\
         kwMDAwMDAwLCJraW5kIjoiYWRkX25vZGUiLCJrdiI6MSwicGFyYW1zIjp7ImZsYXZvciI6ImxhcmdlIiwiaW1h\
         Z2UiOiJ1YnVudHUiLCJrOHNfdmVyc2lvbiI6InYxLjM0LjMrcmtlMnIxIiwibGFiZWxzIjp7ImFwcCI6Indl\
         YiIsInRlYW0uZXhhbXBsZS5jb20vb3duZXIiOiJvcHMiLCJ6b25lIjoiYiJ9LCJwb29sIjoiZ3B1IiwidGFp\
         bnRzIjpbImdwdT10cnVlOk5vU2NoZWR1bGUiLCJkZWRpY2F0ZWQ6UHJlZmVyTm9TY2hlZHVsZSJdfSwicmVx\
         IjoiM2YyYjhjMWUtOWQ0YS00YjZmLThlMjEtNWM3YTliMGQxZTJmIiwidiI6MX0="
    );
    assert_eq!(
        sign_vector(42, &params, uuid, 1_790_000_000),
        json!({
            "request_uuid": uuid,
            "issued_at": 1_790_000_000,
            "expires_at": 1_790_003_000,
            "signature": "Fr25r3tso7d8TRqcBHaFMaBvlZXjVUpzZR+n6ma4xVdR7wNyR0UfWKxBFTCCHFHzLGe17c/\
                          uzfJJMSokr+pUAQ==",
        })
    );
}

#[test]
fn a_replacement_signs_the_pool_params_with_replaces_added() {
    let uuid = "a0b1c2d3-e4f5-4a6b-8c7d-9e0f1a2b3c4d";
    let mut params = object(json!({
        "pool": "workers",
        "flavor": "medium",
        "k8s_version": "v1.34.3+rke2r1",
        "image": "ubuntu",
        "labels": {},
        "taints": [],
    }));
    params.insert("replaces".to_owned(), json!("workers-1a2b3c4d"));
    assert_eq!(
        b64(envelope_bytes(7, uuid, &params, 1_790_000_100).as_bytes()),
        "eyJjbHVzdGVyIjoiNyIsImRvbSI6ImhpcHBpdXMtazhzLW9wIiwiZXhwIjoxNzkwMDAzMTAwLCJpYXQiOjE3OT\
         AwMDAxMDAsImtpbmQiOiJhZGRfbm9kZSIsImt2IjoxLCJwYXJhbXMiOnsiZmxhdm9yIjoibWVkaXVtIiwiaW1h\
         Z2UiOiJ1YnVudHUiLCJrOHNfdmVyc2lvbiI6InYxLjM0LjMrcmtlMnIxIiwibGFiZWxzIjp7fSwicG9vbCI6In\
         dvcmtlcnMiLCJyZXBsYWNlcyI6IndvcmtlcnMtMWEyYjNjNGQiLCJ0YWludHMiOltdfSwicmVxIjoiYTBiMWMy\
         ZDMtZTRmNS00YTZiLThjN2QtOWUwZjFhMmIzYzRkIiwidiI6MX0="
    );
    assert_eq!(
        sign_vector(7, &params, uuid, 1_790_000_100)["signature"],
        "eF+Wh8JIGr1YG/+O8PVaOsQWqIxNkkSy/e6MIBe+uruMoBNwrNGewapesfLdAXnSTkpHm4n2Bs3WXpZaygIVAQ=="
    );
}

/// Non-ASCII keys and values, nested objects in arrays, and every escape. The keys `～`
/// (U+FF5E) and `😀` (U+1F600) sort one way by code point, which Python uses, and the other
/// way by UTF-16 unit.
#[test]
fn non_ascii_nesting_and_escapes_sign_exactly_as_pynacl_signs_them() {
    let uuid = "00000000-0000-4000-8000-000000000001";
    let params = object(json!({
        "pool": "p",
        "labels": {
            "\u{ff5e}": "fullwidth",
            "\u{1f600}": "emoji",
            "équipe": "café-ü",
            "b": { "z": 1, "a": [3, { "y": true, "x": null }] },
            "esc": "q\"b\\n\n\t\u{1}\u{7f}\u{2028}/",
        },
        "taints": [],
    }));
    assert_eq!(
        b64(envelope_bytes(1_234_567, uuid, &params, 1_790_000_200).as_bytes()),
        "eyJjbHVzdGVyIjoiMTIzNDU2NyIsImRvbSI6ImhpcHBpdXMtazhzLW9wIiwiZXhwIjoxNzkwMDAzMjAwLCJpYX\
         QiOjE3OTAwMDAyMDAsImtpbmQiOiJhZGRfbm9kZSIsImt2IjoxLCJwYXJhbXMiOnsibGFiZWxzIjp7ImIiOnsi\
         YSI6WzMseyJ4IjpudWxsLCJ5Ijp0cnVlfV0sInoiOjF9LCJlc2MiOiJxXCJiXFxuXG5cdFx1MDAwMX/igKgvIi\
         wiw6lxdWlwZSI6ImNhZsOpLcO8Iiwi772eIjoiZnVsbHdpZHRoIiwi8J+YgCI6ImVtb2ppIn0sInBvb2wiOiJw\
         IiwidGFpbnRzIjpbXX0sInJlcSI6IjAwMDAwMDAwLTAwMDAtNDAwMC04MDAwLTAwMDAwMDAwMDAwMSIsInYiOj\
         F9"
    );
    assert_eq!(
        sign_vector(1_234_567, &params, uuid, 1_790_000_200)["signature"],
        "OVbpJx6bp41B08M131iRRWpuzriitOVzc4/4pmv58FFTA4BNYke464VfMEjsAl+RNxeuHJI8w4f4gHHhboXDBA=="
    );
}

#[test]
fn each_signed_add_node_has_a_fresh_v4_request_id_and_verifies() {
    let key = vector_key();
    let matched = key.matched(42, &cluster(Some(GUEST))).unwrap();
    let params = add_node_params();
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let first = matched
        .sign_add_node(&params, &Response::dated(None))
        .unwrap();
    let second = matched
        .sign_add_node(&params, &Response::dated(None))
        .unwrap();
    assert_ne!(first["request_uuid"], second["request_uuid"]);
    for signed in [first, second] {
        let uuid = signed["request_uuid"].as_str().unwrap();
        assert_eq!(uuid.len(), 36, "{uuid}");
        assert_eq!(&uuid[14..15], "4", "{uuid}");
        assert!("89ab".contains(&uuid[19..20]), "{uuid}");
        assert!(
            uuid.bytes()
                .all(|b| b == b'-' || b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        let iat = signed["issued_at"].as_u64().unwrap();
        assert!(iat >= before);
        assert_eq!(signed["expires_at"].as_u64().unwrap(), iat + 3000);
        let mut signature = [0_u8; 64];
        Base64::decode(signed["signature"].as_str().unwrap(), &mut signature).unwrap();
        let bytes = envelope_bytes(42, uuid, &params, iat);
        key.sign
            .verifying_key()
            .verify_strict(bytes.as_bytes(), &Signature::from_bytes(&signature))
            .unwrap();
    }
}

/// `seconds` since the epoch as an IMF-fixdate, the inverse of [`http_date`], by Howard
/// Hinnant's `civil_from_days`; 1970-01-01 was a Thursday.
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

/// The local clock at `seconds` since the epoch.
fn clock_at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + std::time::Duration::from_secs(seconds)
}

/// An `add_node` is signed at the time of the answer's `Date` when it has one near the local
/// clock, and refused when the `Date` is far from it.
#[test]
fn an_add_node_is_signed_at_the_server_time_of_the_answer() {
    let key = vector_key();
    let matched = key.matched(42, &cluster(Some(GUEST))).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // The formatter is held to the parser, so the dates below are what they say.
    for seconds in [
        0,
        784_111_777,
        951_825_600,
        1_483_228_800,
        4_107_542_400,
        now,
    ] {
        assert_eq!(http_date(&imf_fixdate(seconds)), Some(seconds), "{seconds}");
    }
    assert_eq!(imf_fixdate(784_111_777), "Sun, 06 Nov 1994 08:49:37 GMT");
    let server = now - 120;
    let answered = Response::dated(Some(&imf_fixdate(server)));
    let signed = matched
        .sign_add_node(&add_node_params(), &answered)
        .unwrap();
    assert_eq!(signed["issued_at"], server);
    assert_eq!(signed["expires_at"], server + 3000);
    let far = Response::dated(Some("Sun, 06 Nov 1994 08:49:37 GMT"));
    let error = matched.sign_add_node(&add_node_params(), &far).unwrap_err();
    assert_eq!(error.code(), Code::Internal);
    assert!(
        error
            .message()
            .contains("`Sun, 06 Nov 1994 08:49:37 GMT` and the local clock disagree"),
        "{error}"
    );
}

/// The `Date` dates the signature when it is within five minutes of the local clock, either
/// way; a `Date` further off is refused, not replaced by the local clock; without a readable
/// `Date` the local clock signs.
#[test]
fn the_issue_time_is_the_server_date_within_five_minutes_then_the_local_clock() {
    let local = clock_at(951_825_600);
    let date = "Tue, 29 Feb 2000 12:00:00 GMT";
    assert_eq!(issued_at(Some(date), local).unwrap(), 951_825_600);
    for (date, seconds) in [
        ("Tue, 29 Feb 2000 12:05:00 GMT", 951_825_900),
        ("Tue, 29 Feb 2000 11:55:00 GMT", 951_825_300),
    ] {
        assert_eq!(issued_at(Some(date), local).unwrap(), seconds, "{date}");
    }
    assert_eq!(issued_at(None, local).unwrap(), 951_825_600);
    assert_eq!(issued_at(Some("yesterday"), local).unwrap(), 951_825_600);
    for date in [
        "Tue, 29 Feb 2000 12:06:00 GMT",
        "Tue, 29 Feb 2000 11:54:00 GMT",
        "Sun, 06 Nov 1994 08:49:37 GMT",
    ] {
        let error = issued_at(Some(date), local).unwrap_err();
        assert_eq!(error.code(), Code::Internal, "{date}");
        assert_eq!(
            error.message(),
            format!(
                "cannot sign a node operation: the API's `Date` `{date}` and the local clock \
                 disagree by more than 5 minutes, so no signature is dated by either"
            )
        );
        assert_eq!(error.hint(), Some(CLOCK_HINT));
    }
    // A clock before 1970 has no `iat`, and disagrees with any `Date` a server sends.
    let before = UNIX_EPOCH - std::time::Duration::from_secs(1);
    let error = issued_at(Some(date), before).unwrap_err();
    assert!(error.message().contains("disagree"), "{error}");
    let error = issued_at(None, before).unwrap_err();
    assert_eq!(error.code(), Code::Internal);
    assert_eq!(
        error.message(),
        "cannot sign a node operation: the system clock is set before 1970, and the API's \
         answer had no `Date` to sign by"
    );
    assert_eq!(error.hint(), Some(CLOCK_HINT));
}

/// The one hint of both refusals to date a signature: the clock to check, and the report
/// an `internal` error asks for.
const CLOCK_HINT: &str = "check the system clock, then run this again with `--debug` and \
                          report what it prints at \
                          `https://github.com/thenervelab/hippius-cli/issues`";

#[test]
fn an_http_date_is_read_in_the_imf_fixdate_form_only() {
    for (date, seconds) in [
        ("Thu, 01 Jan 1970 00:00:00 GMT", 0),
        ("Sun, 06 Nov 1994 08:49:37 GMT", 784_111_777),
        ("Tue, 29 Feb 2000 12:00:00 GMT", 951_825_600),
        ("Sun, 01 Nov 2026 23:59:59 GMT", 1_793_577_599),
        ("Mon, 01 Mar 2100 00:00:00 GMT", 4_107_542_400),
        // A leap second, which IMF-fixdate allows.
        ("Wed, 31 Dec 2016 23:59:60 GMT", 1_483_228_800),
    ] {
        assert_eq!(http_date(date), Some(seconds), "{date}");
    }
    for date in [
        "",
        "Sunday, 06-Nov-94 08:49:37 GMT",
        "Sun Nov  6 08:49:37 1994",
        "Sun, 06 Nov 1994 08:49:37 UTC",
        "Sun, 06 Nov 1994 08:49:37 GMT ",
        "Sun, 06 Nov 1994 08:49 GMT",
        "Sun, 06 Nov 1994 08:49:37:00 GMT",
        "Sun, 06 Now 1994 08:49:37 GMT",
        "Sun, 6 Nov 1994 08:49:37 GMT",
        "Sun, +6 Nov 1994 08:49:37 GMT",
        "Sun, 06 Nov 94 08:49:37 GMT",
        "Sun, 06 Nov 1994 8:49:37 GMT",
        "Sun, 06 Nov 1994 08:4x:37 GMT",
        "Sun, 06 Nov 1994 08:49:3 GMT",
        "Wed, 31 Dec 1969 23:59:59 GMT",
        "Sun, 00 Nov 1994 08:49:37 GMT",
        "Sun, 32 Nov 1994 08:49:37 GMT",
        "Sun, 31 Nov 1994 08:49:37 GMT",
        "Thu, 29 Feb 2001 00:00:00 GMT",
        "Mon, 29 Feb 2100 00:00:00 GMT",
        "Thu, 30 Feb 2000 00:00:00 GMT",
        "Sun, 06 Nov 1994 24:00:00 GMT",
        "Sun, 06 Nov 1994 08:60:00 GMT",
        "Sun, 06 Nov 1994 08:49:61 GMT",
    ] {
        assert_eq!(http_date(date), None, "{date}");
    }
}

/// Every day from 1970 to 2400 is one day after the one before it, by a calendar written
/// here from the Gregorian leap rule.
#[test]
fn each_day_of_the_calendar_is_one_day_after_the_last() {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut expected = 0;
    for year in 1970..=2400 {
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        for (month, name) in MONTHS.iter().enumerate() {
            let days = match month {
                1 if leap => 29,
                1 => 28,
                3 | 5 | 8 | 10 => 30,
                _ => 31,
            };
            for day in 1..=days {
                let date = format!("Mon, {day:02} {name} {year} 00:00:00 GMT");
                assert_eq!(http_date(&date), Some(expected), "{date}");
                expected += 86_400;
            }
        }
    }
}

#[test]
fn a_uuid_gets_the_version_and_variant_bits() {
    assert_eq!(uuid_v4([0; 16]), "00000000-0000-4000-8000-000000000000");
    assert_eq!(uuid_v4([0xff; 16]), "ffffffff-ffff-4fff-bfff-ffffffffffff");
}

#[test]
fn canonical_json_refuses_what_python_prints_differently() {
    for value in [json!(1.5), json!(1e3), json!({ "a": [0.0] })] {
        assert_eq!(canonical(&value), None, "{value}");
    }
    let minus_zero: Value = serde_json::from_str("-0").unwrap();
    assert_eq!(canonical(&minus_zero), None);
    let big: Value = serde_json::from_str("123456789012345678901234567890").unwrap();
    assert_eq!(
        canonical(&json!([big, -5, false, null])).unwrap(),
        "[123456789012345678901234567890,-5,false,null]"
    );
    let key = vector_key();
    let matched = key.matched(42, &cluster(Some(GUEST))).unwrap();
    let params = object(json!({ "count": 1.5 }));
    let error = matched
        .sign_add_node(&params, &Response::dated(None))
        .unwrap_err();
    assert_eq!(error.code(), Code::Internal);
    assert_eq!(
        error.message(),
        "cannot sign a node for the Kubernetes cluster `prod` (`42`): the pool's \
         `signing_params` hold a number that is not an integer"
    );
}

#[test]
fn a_string_is_escaped_as_python_escapes_it() {
    let every_control: String = (0_u8..0x20).map(char::from).collect();
    let mut out = String::new();
    python_string(&format!("{every_control}\"\\\u{7f}é/"), &mut out);
    assert_eq!(
        out,
        "\"\\u0000\\u0001\\u0002\\u0003\\u0004\\u0005\\u0006\\u0007\\b\\t\\n\\u000b\\f\\r\
         \\u000e\\u000f\\u0010\\u0011\\u0012\\u0013\\u0014\\u0015\\u0016\\u0017\\u0018\\u0019\
         \\u001a\\u001b\\u001c\\u001d\\u001e\\u001f\\\"\\\\\u{7f}é/\""
    );
}

/// Every character, U+0000 to U+10FFFF, is written as Python's `json.dumps(ensure_ascii=False)`
/// writes it. `serde_json` writes a string by the same rules (the two-character escapes, every
/// other control below U+0020 as lowercase `\u00xx`, everything else as it is), so it is the
/// oracle here; the pinned test above holds both to Python's own output.
#[test]
fn every_character_is_escaped_as_python_escapes_it() {
    for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
        let text = c.to_string();
        let mut out = String::new();
        python_string(&text, &mut out);
        assert_eq!(
            out,
            serde_json::to_string(&text).unwrap(),
            "U+{:04X}",
            u32::from(c)
        );
    }
}

/// The refusal of a key file `text`, and that its message and hint quote none of it.
fn refused(name: &str, text: &[u8]) -> Error {
    let error = load_text(name, text, 0o600).unwrap_err();
    assert_eq!(error.code(), Code::Usage, "{name}: {}", error.message());
    let secret = b64(&range(11, 43));
    for said in [error.message(), error.hint().unwrap_or("")] {
        assert!(!said.contains(&secret), "{name}: {said}");
        assert!(!said.contains(&b64(&range(1, 33))), "{name}: {said}");
    }
    error
}

/// The vector key file with `field` set to `value`.
fn with(field: &str, value: Value) -> String {
    let mut file: Value = serde_json::from_str(&vector_key_file()).unwrap();
    file[field] = value;
    file.to_string()
}

/// The vector key file without `field`.
fn without(field: &str) -> String {
    let mut file: Value = serde_json::from_str(&vector_key_file()).unwrap();
    file.as_object_mut().unwrap().shift_remove(field);
    file.to_string()
}

/// Files that are not a version-1 key file: a name, the text, and what the refusal says.
fn not_key_files() -> Vec<(&'static str, String, &'static str)> {
    let sign = b64(&range(11, 43));
    let boxed = b64(&range(1, 33));
    vec![
        (
            "ssh",
            "-----BEGIN OPENSSH PRIVATE KEY-----".to_owned(),
            "is not a JSON object",
        ),
        ("array", "[]".to_owned(), "is not a JSON object"),
        (
            "repeated",
            vector_key_file()
                .trim_end()
                .strip_suffix('}')
                .unwrap()
                .to_owned()
                + r#","version":1}"#,
            "names a field twice",
        ),
        (
            "kind",
            with("kind", json!("hippius-drive-key")),
            "has no `kind` of",
        ),
        ("no-kind", without("kind"), "has no `kind` of"),
        (
            "version-2",
            with("version", json!(2)),
            "is not of version `1`",
        ),
        (
            "version-text",
            with("version", json!("1")),
            "is not of version `1`",
        ),
        (
            "version-float",
            with("version", json!(1.0)),
            "is not of version `1`",
        ),
        ("no-version", without("version"), "is not of version `1`"),
        (
            "no-secret",
            without("sign_secret_key"),
            "has no `sign_secret_key` of 32 bytes",
        ),
        (
            "number",
            with("box_secret_key", json!(7)),
            "has no `box_secret_key` of 32 bytes",
        ),
        (
            "not-b64",
            with("sign_secret_key", json!("not base64!")),
            "`sign_secret_key`",
        ),
        (
            "short",
            with("sign_secret_key", json!(b64(&[1; 31]))),
            "`sign_secret_key`",
        ),
        (
            "long",
            with("box_secret_key", json!(b64(&[1; 33]))),
            "`box_secret_key`",
        ),
        // Without its padding, and with whitespace, base64 is not canonical.
        (
            "unpadded",
            with("sign_secret_key", json!(sign.trim_end_matches('='))),
            "`sign_secret_key`",
        ),
        (
            "space",
            with("box_secret_key", json!(format!(" {boxed}"))),
            "`box_secret_key`",
        ),
        ("no-public", without("box_public_key"), "`box_public_key`"),
        (
            "swapped",
            with("box_public_key", json!(GUEST)),
            "is damaged",
        ),
        (
            "other-sign",
            with("sign_public_key", json!(GUEST)),
            "is damaged",
        ),
    ]
}

#[test]
fn a_file_that_is_not_a_version_1_key_file_is_usage() {
    for (name, text, says) in not_key_files() {
        let error = refused(name, text.as_bytes());
        assert!(
            error.message().contains(says),
            "{name}: {}",
            error.message()
        );
        if name.starts_with("version") || name == "no-version" {
            assert_eq!(
                error.hint(),
                Some("check that the file came from the console, or update hipc")
            );
        } else {
            assert_eq!(
                error.hint(),
                Some(
                    "pass `--cluster-key-file` the `hippius-cluster-<name>.key` file the \
                     console saved when it created the cluster"
                ),
                "{name}"
            );
        }
    }
    let error = refused("whole", without("kind").as_bytes());
    assert!(
        error.message().starts_with("the cluster key file `")
            && error.message().ends_with(
                "/hippius-cluster-prod.key` has no `kind` of `hippius-k8s-cluster-key`, so it is \
                 not a key file the console saved"
            ),
        "{}",
        error.message()
    );
}

#[test]
fn the_key_file_is_held_to_the_checks_of_the_config_file() {
    let dir = temp_dir("checks");
    let load = |path: &Path| ClusterKey::load(path, "--cluster-key-file").unwrap_err();
    // A path that names no file is not repeated: it may be the key passed in its place.
    for missing in [dir.join("missing.key"), dir.join("no-dir").join("x.key")] {
        let error = load(&missing);
        assert_eq!(error.code(), Code::Usage);
        assert_eq!(
            error.message(),
            "the cluster key file `--cluster-key-file` names does not exist; the path is not \
             repeated here, in case it is a secret passed in place of the file"
        );
        assert!(
            error
                .hint()
                .is_some_and(|hint| hint.starts_with("pass `--cluster-key-file` the"))
        );
    }
    let file = dir.join("hippius-cluster-prod.key");
    fs::write(&file, vector_key_file()).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    let error = load(&file);
    assert!(
        error.message().ends_with(
            "has permission bits for the group or others, and it holds the cluster's private keys"
        ),
        "{}",
        error.message()
    );
    assert!(
        error.hint().unwrap().starts_with("run `chmod 600 "),
        "{error:?}"
    );
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.join("link.key");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    let error = load(&link);
    assert!(
        error.message().contains("is a symlink"),
        "{}",
        error.message()
    );
    assert_eq!(
        error.hint(),
        Some("pass `--cluster-key-file` the file the symlink points to")
    );
    let error = load(&dir);
    assert_eq!(error.code(), Code::Usage);
    let big = dir.join("big.key");
    fs::write(&big, vec![b' '; (1 << 20) + 1]).unwrap();
    fs::set_permissions(&big, fs::Permissions::from_mode(0o600)).unwrap();
    let error = load(&big);
    assert!(
        error.message().ends_with("is larger than 1 MiB"),
        "{}",
        error.message()
    );
    assert!(
        error
            .hint()
            .unwrap()
            .contains("or pass `--cluster-key-file` another file")
    );
    ClusterKey::load(&file, "--cluster-key-file").unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn debug_shows_the_public_keys_and_never_a_secret_key() {
    let key = vector_key();
    let matched = key.matched(42, &cluster(Some(GUEST))).unwrap();
    for shown in [format!("{key:?}"), format!("{matched:?}")] {
        assert!(
            shown.contains(SIGN_PUBLIC) && shown.contains(BOX_PUBLIC),
            "{shown}"
        );
        // A path is user data, which `Debug` does not print.
        assert!(!shown.contains("path"), "{shown}");
        for secret in [range(11, 43), range(1, 33)] {
            assert!(!shown.contains(&b64(&secret)), "{shown}");
            assert!(!shown.contains(&format!("{secret:?}")), "{shown}");
            assert!(!shown.contains(&hex(&secret)), "{shown}");
        }
    }
}

#[test]
fn no_random_bytes_is_internal_and_quotes_the_system() {
    let error = no_random_bytes(getrandom::Error::UNSUPPORTED);
    assert_eq!(error.code(), Code::Internal);
    assert!(
        error
            .message()
            .starts_with("cannot read random bytes for a request id: the system says `"),
        "{}",
        error.message()
    );
}
