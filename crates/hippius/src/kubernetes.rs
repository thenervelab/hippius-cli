//! The cluster key of a managed Kubernetes cluster, read from the key file the console saves
//! when it creates the cluster (`hippius-cluster-<name>.key`).
//!
//! At create the console makes two key pairs in the browser and sends only their public
//! halves: an Ed25519 signing pair and an X25519 box pair. The cluster's guest seals the
//! kubeconfig to the box key, as a libsodium sealed box, and signs each such drop with a
//! signing key of its own, `guest_sign_pubkey`. The masters accept a new node only with an
//! Ed25519 signature by the client signing key. So this module does two things with the
//! file: open the kubeconfig, and sign the `add_node` operations of a scale-up or a
//! replacement. Creating a cluster, and so generating a key, stays in the console.
//!
//! # The console's key file format, version 1
//!
//! A JSON object with `kind` `"hippius-k8s-cluster-key"`, `version` `1`, and four standard
//! base64 keys of 32 bytes each: `sign_secret_key` (the Ed25519 seed), `sign_public_key`,
//! `box_secret_key` (the X25519 secret), and `box_public_key`. `cluster_name` and
//! `created_at` are informational. Each public key is derived again from its secret and must
//! match, so a hand-edited file fails here and not later as a box that will not open. The
//! file gets the checks of the config file (owner-only mode, no symlink, at most 1 MiB).
//!
//! # The bytes that are signed and opened
//!
//! - A drop is verified before it is opened. Its signature is Ed25519, by the guest key, over
//!   the ASCII `hippius-k8s-drop:v1:<cluster id>:<kind>:<hex SHA-256 of the base64 ciphertext
//!   text>`. Its ciphertext is `crypto_box_seal` to the box key: `epk(32) ||
//!   XSalsa20-Poly1305(m)`, with the nonce `BLAKE2b-24(epk || pk)`.
//! - An `add_node` signature is Ed25519, by the client signing key, over the canonical JSON of
//!   `{v: 1, dom: "hippius-k8s-op", cluster: "<id>", req: <lowercase v4 UUID>, kind:
//!   "add_node", params, iat, exp, kv: 1}`, where `params` is the pool's `signing_params` as
//!   the API returns them, plus `replaces: "<node>"` for a replacement. Canonical JSON is
//!   Python's `json.dumps(sort_keys=True, separators=(",", ":"), ensure_ascii=False)` in
//!   UTF-8, since the backend builds the same bytes that way. `exp` is `iat` plus 3000
//!   seconds, under the backend's one-hour cap. Only `{request_uuid, issued_at, expires_at,
//!   signature}` is sent.
//!
//! The tests hold this module to the console's known-answer vectors, which the console made
//! with the backend's own code (PyNaCl): if one stops passing, a real cluster refuses what
//! `hipc` signs.
//!
//! # Secrets
//!
//! The private keys never leave the process: nothing here returns, prints, logs, or sends
//! them, and no error or `Debug` output quotes the file. The file's bytes, the secret key
//! texts, and the decoded secret keys are cleared once the keys are built or the file is
//! refused. The Ed25519 key clears its seed when dropped (ed25519-dalek's `zeroize` feature);
//! the X25519 secret is kept in memory that is cleared on drop, and the `crypto_box` key made
//! of it for each use is not cleared ([`box_key`]). That is best effort: the JSON parser may
//! copy a string it unescapes, and the clearing does not reach such a copy, nor the copies a
//! move leaves on the stack.

use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64ct::{Base64, Encoding as _};
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::client::Response;
use crate::dns::{ObjectFile, load_object};
use crate::error::{Code, Error, Result};
use crate::phrase::{named, quote, quote_path};
use crate::private_file::{Guarded, NamedBy};

/// The `kind` of the key file.
const KIND: &str = "hippius-k8s-cluster-key";

/// The one `version` of the key file this module reads.
const VERSION: u64 = 1;

/// The bytes of every key in the file.
const KEY_BYTES: usize = 32;

/// The prefix of a drop's signed message.
const DROP_DOMAIN: &str = "hippius-k8s-drop:v1";

/// How long a signed operation stays valid, in seconds: the lifetime the console gives its
/// own signatures, under the backend's one-hour cap with room for a slow queue.
const ENVELOPE_LIFETIME_SECS: u64 = 3000;

/// The key of a cluster, loaded from its key file. Its `Debug` shows the public keys only.
pub struct ClusterKey {
    sign: SigningKey,
    /// The X25519 secret, kept here rather than in a `crypto_box::SecretKey` ([`box_key`]).
    box_secret: Zeroizing<[u8; KEY_BYTES]>,
    /// The two public keys in standard base64, as the cluster records them.
    sign_public: String,
    box_public: String,
    /// The file it came from, and the flag that named it, for messages.
    path: PathBuf,
    flag: &'static str,
}

impl ClusterKey {
    /// Reads the key file at `path`, named by `flag`, and checks it: the file as the config
    /// file is checked, then its format (module docs). Sends nothing.
    ///
    /// # Errors
    ///
    /// `usage` for a file that is missing, a symlink, not a regular file, another user's, open
    /// to the group or others, larger than 1 MiB, or not a version-1 key file whose public keys
    /// match its secret keys. No message quotes what the file holds.
    pub fn load(path: &Path, flag: &'static str) -> Result<Self> {
        let guarded = Guarded {
            what: "the cluster key file",
            holds: "the cluster's private keys",
            named_by: NamedBy::Flag(flag),
        };
        let loaded = load_object(path, guarded, &key_file_hint(flag))?;
        let not_a_key_file = |problem: &str| {
            Error::usage(
                format!(
                    "the cluster key file {} {problem}, so it is not a key file the console \
                     saved",
                    quote_path(path)
                ),
                key_file_hint(flag),
            )
        };
        // A field given twice is refused, as the other private files refuse it: which copy
        // counts would depend on the parser.
        let mut fields = match loaded {
            ObjectFile::Object(fields) => fields,
            ObjectFile::Repeated => return Err(not_a_key_file("names a field twice")),
            ObjectFile::NotAnObject => return Err(not_a_key_file("is not a JSON object")),
        };
        // Taken out of the map first, so that each refusal below drops them cleared.
        let sign_secret = secret_text(&mut fields, "sign_secret_key");
        let box_secret = secret_text(&mut fields, "box_secret_key");
        if fields.get("kind").and_then(Value::as_str) != Some(KIND) {
            return Err(not_a_key_file(&format!("has no `kind` of `{KIND}`")));
        }
        if fields.get("version").and_then(Value::as_u64) != Some(VERSION) {
            return Err(Error::usage(
                format!(
                    "the cluster key file {} is not of version `{VERSION}`, the one this hipc \
                     reads",
                    quote_path(path)
                ),
                "check that the file came from the console, or update hipc",
            ));
        }
        let key = |field: &str, text: Option<&str>| {
            text.and_then(decode_key).ok_or_else(|| {
                not_a_key_file(&format!(
                    "has no `{field}` of {KEY_BYTES} bytes in standard base64"
                ))
            })
        };
        let public = |field: &str| key(field, fields.get(field).and_then(Value::as_str));
        let sign_secret = key(
            "sign_secret_key",
            sign_secret.as_deref().map(String::as_str),
        )?;
        let box_secret = key("box_secret_key", box_secret.as_deref().map(String::as_str))?;
        let sign_public = public("sign_public_key")?;
        let box_public = public("box_public_key")?;
        let sign = SigningKey::from_bytes(&sign_secret);
        // Compared as public data: neither public key is a secret.
        if sign.verifying_key().as_bytes() != &*sign_public
            || box_key(&box_secret).public_key().as_bytes() != &*box_public
        {
            return Err(Error::usage(
                format!(
                    "the cluster key file {} is damaged: its public keys do not match its \
                     secret keys",
                    quote_path(path)
                ),
                key_file_hint(flag),
            ));
        }
        Ok(Self {
            sign,
            box_secret,
            sign_public: Base64::encode_string(&*sign_public),
            box_public: Base64::encode_string(&*box_public),
            path: path.to_owned(),
            flag,
        })
    }

    /// The key, checked against `cluster`, the cluster `id` as the API returned it: both
    /// public keys must be the ones it was created with, as the console checks. The box key
    /// opens the kubeconfig and the signing key consents to node operations, so a file that
    /// matches on one only is another cluster's.
    ///
    /// # Errors
    ///
    /// `usage` when the key file is another cluster's, and `internal` when `cluster` has no
    /// `client_sign_pubkey` or `client_box_pubkey`.
    pub fn matched(&self, id: u64, cluster: &Value) -> Result<MatchedKey<'_>> {
        let named = named(id, cluster);
        let public = |field: &str| {
            cluster.get(field).and_then(Value::as_str).ok_or_else(|| {
                Error::internal(format!(
                    "cannot check the cluster key file against the Kubernetes cluster \
                     {named}: the API answered without its `{field}`"
                ))
            })
        };
        if public("client_sign_pubkey")? != self.sign_public
            || public("client_box_pubkey")? != self.box_public
        {
            return Err(Error::usage(
                format!(
                    "the cluster key file {} is not the key of the Kubernetes cluster {named}: \
                     its public keys are not the ones the cluster was created with",
                    quote_path(&self.path)
                ),
                key_file_hint(self.flag),
            ));
        }
        Ok(MatchedKey {
            key: self,
            id,
            guest: cluster
                .get("guest_sign_pubkey")
                .and_then(Value::as_str)
                .map(str::to_owned),
            named,
        })
    }
}

/// Shows the public keys only: never a secret key, nor the path, which is user data.
impl fmt::Debug for ClusterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClusterKey")
            .field("sign_public", &self.sign_public)
            .field("box_public", &self.box_public)
            .finish_non_exhaustive()
    }
}

/// A [`ClusterKey`] that [`ClusterKey::matched`] checked against one cluster: the only way to
/// open that cluster's kubeconfig or sign its node operations.
#[derive(Debug)]
pub struct MatchedKey<'a> {
    key: &'a ClusterKey,
    id: u64,
    /// The cluster as a message names it: `` `prod` (`7`) ``.
    named: String,
    /// The cluster's `guest_sign_pubkey`, which signs its drops; `None` until it publishes
    /// one.
    guest: Option<String>,
}

impl MatchedKey<'_> {
    /// Opens the cluster's kubeconfig from `secrets`, the answer of `GET
    /// /api/kubernetes/clusters/<id>/secrets/`, as the console does: the newest kubeconfig drop
    /// made at launch (no `operation`), its signature verified against the cluster's guest key
    /// before anything is decrypted.
    ///
    /// # Errors
    ///
    /// `not_found` while the cluster has no guest key or no kubeconfig drop yet. `internal`
    /// when the two reads disagree on the guest key, when the signature does not verify, when
    /// the box does not open with this key, or when what it holds is not UTF-8 text: each is
    /// an answer `hipc` refuses to trust, and nothing is returned.
    pub fn open_kubeconfig(&self, secrets: &Value) -> Result<Zeroizing<String>> {
        let named = &self.named;
        let not_yet = |what: &str| {
            Error::new(
                Code::NotFound,
                format!("the Kubernetes cluster {named} has not {what} yet"),
            )
            .with_hint(
                "wait for the cluster to finish forming, which seals the kubeconfig to the \
                 cluster key 5 to 10 minutes after its masters are up, then run this again",
            )
        };
        let Some(guest) = &self.guest else {
            return Err(not_yet("published the key that signs its kubeconfig"));
        };
        if secrets.get("guest_sign_pubkey").and_then(Value::as_str) != Some(guest) {
            return Err(Error::internal(format!(
                "the key that signs the secrets of the Kubernetes cluster {named} changed \
                 between two reads, so nothing was decrypted"
            )));
        }
        let Some((ciphertext, signature)) = launch_drop(secrets, "kubeconfig") else {
            return Err(not_yet("handed over its kubeconfig"));
        };
        let refused = |why: &str| {
            Error::internal(format!(
                "the kubeconfig of the Kubernetes cluster {named} {why}, so it was refused"
            ))
        };
        if !verify_drop(guest, self.id, "kubeconfig", ciphertext, signature) {
            return Err(refused(
                "is not signed by the cluster's guest key, and it was not decrypted",
            ));
        }
        let sealed = Base64::decode_vec(ciphertext).map_err(|_| refused("is not base64"))?;
        let opened = box_key(&self.key.box_secret).unseal(&sealed).map_err(|_| {
            refused(&format!(
                "does not open with the cluster key file {}",
                quote_path(&self.key.path)
            ))
        })?;
        String::from_utf8(opened)
            .map(Zeroizing::new)
            .map_err(|error| {
                drop(Zeroizing::new(error.into_bytes()));
                refused("opened, but it is not UTF-8 text")
            })
    }

    /// One `add_node` operation for the cluster, signed now with a new request id, as
    /// `{request_uuid, issued_at, expires_at, signature}`. `params` are the pool's
    /// `signing_params` as the API returned them, with `replaces` added for a replacement.
    /// `issued_at` is the time of `answered`, the API's answer to the last read before this,
    /// by its `Date` header (or the local clock: `issued_at`).
    ///
    /// # Errors
    ///
    /// `internal` when `params` hold a number that is not an integer, which the backend's
    /// canonical JSON would print differently, when the system gives no random bytes, when
    /// `answered` has a `Date` more than five minutes from the local clock, or when it has no
    /// `Date` and the local clock is set before 1970.
    pub fn sign_add_node(&self, params: &Map<String, Value>, answered: &Response) -> Result<Value> {
        let mut random = [0_u8; 16];
        getrandom::getrandom(&mut random).map_err(no_random_bytes)?;
        let issued_at = issued_at(answered.date(), SystemTime::now())?;
        self.add_node_at(params, &uuid_v4(random), issued_at)
    }

    /// [`MatchedKey::sign_add_node`] with its request id and time given.
    fn add_node_at(
        &self,
        params: &Map<String, Value>,
        request_uuid: &str,
        issued_at: u64,
    ) -> Result<Value> {
        let expires_at = issued_at + ENVELOPE_LIFETIME_SECS;
        let envelope = json!({
            "v": 1,
            "dom": "hippius-k8s-op",
            "cluster": self.id.to_string(),
            "req": request_uuid,
            "kind": "add_node",
            "params": params,
            "iat": issued_at,
            "exp": expires_at,
            "kv": 1,
        });
        let bytes = canonical(&envelope).ok_or_else(|| {
            Error::internal(format!(
                "cannot sign a node for the Kubernetes cluster {}: the pool's `signing_params` \
                 hold a number that is not an integer",
                self.named
            ))
        })?;
        let signature = self.key.sign.sign(bytes.as_bytes());
        Ok(json!({
            "request_uuid": request_uuid,
            "issued_at": issued_at,
            "expires_at": expires_at,
            "signature": Base64::encode_string(&signature.to_bytes()),
        }))
    }
}

/// The `crypto_box` key of `secret`, built for one use and dropped after it. `crypto_box` 0.9.1
/// clears only the scalar of a `SecretKey` when it is dropped, not its copy of the 32 bytes
/// (`impl Drop for SecretKey` in its `src/secret_key.rs`), and the field is private to that
/// crate. So [`ClusterKey`] keeps the bytes in memory it clears, and the copy this makes lives
/// only for the call that uses it: on the stack, where it stays until that memory is reused.
fn box_key(secret: &[u8; KEY_BYTES]) -> crypto_box::SecretKey {
    crypto_box::SecretKey::from_bytes(*secret)
}

/// How far the API's `Date` may be from the local clock, in seconds, and still date a
/// signature: five minutes, the allowance Kerberos and AWS Signature Version 4 give a
/// request's clock.
const CLOCK_SKEW_SECS: u64 = 300;

/// The `iat` of an operation signed now. The backend checks `iat` and `exp` against its own
/// clock, so the server's clock is used when the answer's `Date` holds a time `hipc` reads
/// ([`http_date`]): a local clock that runs fast or slow would otherwise sign a window the
/// backend refuses. Without one it is `now`, the local clock. A `Date` is precise to the
/// second, which is the precision of `iat`.
///
/// A `Date` more than [`CLOCK_SKEW_SECS`] from the local clock is refused rather than used
/// or replaced: a server that answers with a `Date` in the future would be handed a signature
/// it can keep and replay once that time comes, and one that answers with a `Date` in the
/// past is not the server the backend's own check expects. Which clock is wrong is not
/// known here, so neither signs.
fn issued_at(date: Option<&str>, now: SystemTime) -> Result<u64> {
    let local = now.duration_since(UNIX_EPOCH).map(|since| since.as_secs());
    let server = date.and_then(|text| http_date(text).map(|seconds| (text, seconds)));
    match (server, local) {
        (Some((_, server)), Ok(local)) if server.abs_diff(local) <= CLOCK_SKEW_SECS => Ok(server),
        (Some((text, _)), _) => Err(Error::internal(format!(
            "cannot sign a node operation: the API's `Date` {} and the local clock disagree \
             by more than {} minutes, so no signature is dated by either",
            quote(text),
            CLOCK_SKEW_SECS / 60
        ))
        .with_hint(clock_hint())),
        (None, Ok(local)) => Ok(local),
        (None, Err(_)) => Err(Error::internal(
            "cannot sign a node operation: the system clock is set before 1970, and the API's \
             answer had no `Date` to sign by",
        )
        .with_hint(clock_hint())),
    }
}

/// The hint of a refusal to date a signature: the clock is the one thing the caller can fix,
/// and a server whose `Date` is far off is a bug to report, so the hint keeps both halves of
/// the `internal` hint it replaces.
pub(crate) fn clock_hint() -> String {
    format!(
        "check the system clock, then run this again with `--debug` and report what it \
         prints at `{}`",
        crate::error::ISSUES_URL
    )
}

/// Seconds since the Unix epoch of `date`, an HTTP `Date` in the IMF-fixdate form that RFC
/// 9110 makes servers send (section 5.6.7, <https://www.rfc-editor.org/rfc/rfc9110#section-5.6.7>),
/// such as `Sun, 06 Nov 1994 08:49:37 GMT`, or `None` for any other text. The two obsolete
/// forms that section lets a recipient read are left out on purpose: no server `hipc` talks to
/// sends them, and without a time the caller uses its own clock. The day is checked against
/// the month's length (no 31 Feb), and the weekday is not checked against the date.
pub(crate) fn http_date(date: &str) -> Option<u64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_weekday, rest) = date.split_once(", ")?;
    let fields: Vec<&str> = rest.split(' ').collect();
    let [day, month, year, time, "GMT"] = fields[..] else {
        return None;
    };
    let clock: Vec<&str> = time.split(':').collect();
    let [hour, minute, second] = clock[..] else {
        return None;
    };
    let month = MONTHS.iter().position(|m| *m == month)? as u64 + 1;
    let [day, hour, minute, second] = [day, hour, minute, second].map(|text| digits(text, 2));
    let (day, hour, minute, second) = (day?, hour?, minute?, second?);
    civil_seconds(digits(year, 4)?, month, day, [hour, minute, second])
}

/// Seconds since the Unix epoch of a UTC date and time of day, or `None` for a date before
/// 1970 or one that does not exist: a month outside 1 to 12, a day past the month's length
/// (no 31 Feb, and 29 Feb only in a leap year), an hour past 23, or a minute past 59. A second
/// of 60 is a leap second, which both IMF-fixdate and RFC 3339 allow.
fn civil_seconds(year: u64, month: u64, day: u64, [hour, minute, second]: [u64; 3]) -> Option<u64> {
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let last_day = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return None,
    };
    if year < 1970 || !(1..=last_day).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some(days_since_epoch(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// `text` as a number when it is exactly `width` ASCII digits.
fn digits(text: &str, width: usize) -> Option<u64> {
    if text.len() != width || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// The seconds since 1970 of an RFC 3339 time as the API writes it,
/// `YYYY-MM-DDTHH:MM:SS`, an optional fraction, and `Z` or an offset `+HH:MM` or `-HH:MM`;
/// `None` for any other text, a date or an offset that does not exist (`02-31`, `+24:00`), or
/// a time before 1970.
pub fn rfc3339_seconds(text: &str) -> Option<u64> {
    rfc3339(text).map(|(seconds, _)| seconds)
}

/// [`rfc3339_seconds`] with the fraction of a second, in nanoseconds: digits past the ninth
/// are dropped, so two times that differ only there compare equal.
fn rfc3339(text: &str) -> Option<(u64, u32)> {
    let (date, rest) = text.split_once('T')?;
    let (time, zone) = rest.split_at(rest.find(['Z', '+', '-'])?);
    let date: Vec<&str> = date.split('-').collect();
    let [year, month, day] = date[..] else {
        return None;
    };
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let nanos = format!("{:0<9}", &fraction[..fraction.len().min(9)])
        .parse()
        .ok()?;
    let clock: Vec<&str> = clock.split(':').collect();
    let [hour, minute, second] = clock[..] else {
        return None;
    };
    let local = civil_seconds(
        digits(year, 4)?,
        digits(month, 2)?,
        digits(day, 2)?,
        [digits(hour, 2)?, digits(minute, 2)?, digits(second, 2)?],
    )?;
    let offset = if zone == "Z" {
        0
    } else {
        let (sign, hm) = zone.split_at(1);
        let (h, m) = hm.split_once(':')?;
        let (h, m) = (digits(h, 2)?, digits(m, 2)?);
        if h > 23 || m > 59 {
            return None;
        }
        let seconds = i64::try_from(h * 3600 + m * 60).ok()?;
        match sign {
            "+" => seconds,
            "-" => -seconds,
            _ => return None,
        }
    };
    let seconds = u64::try_from(i64::try_from(local).ok()? - offset).ok()?;
    Some((seconds, nanos))
}

/// The days from 1970-01-01 to `year`-`month`-`day` in the proleptic Gregorian calendar, for a
/// year from 1970: Howard Hinnant's `days_from_civil`
/// (<https://howardhinnant.github.io/date_algorithms.html#days_from_civil>), which counts in
/// 400-year eras of 146097 days from a year that starts in March, so that a leap day falls at
/// the end of its year. 719468 is the day of 1970-01-01 in that count.
fn days_since_epoch(year: u64, month: u64, day: u64) -> u64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The refusal when the system gives no random bytes for a request id.
fn no_random_bytes(error: getrandom::Error) -> Error {
    Error::internal(format!(
        "cannot read random bytes for a request id: the system says {}",
        quote(error)
    ))
}

/// The hint of every refused key file.
fn key_file_hint(flag: &str) -> String {
    format!(
        "pass `{flag}` the `hippius-cluster-<name>.key` file the console saved when it created \
         the cluster"
    )
}

/// Takes the string `field` out of `fields`, to be cleared when dropped, or `None` when it is
/// missing or not a string, which is cleared now.
fn secret_text(fields: &mut Map<String, Value>, field: &str) -> Option<Zeroizing<String>> {
    match fields.shift_remove(field)? {
        Value::String(text) => Some(Zeroizing::new(text)),
        other => {
            crate::dns::clear_owned(other);
            None
        }
    }
}

/// `text` decoded from standard base64 into exactly [`KEY_BYTES`] bytes, or `None`.
fn decode_key(text: &str) -> Option<Zeroizing<[u8; KEY_BYTES]>> {
    let mut key = Zeroizing::new([0_u8; KEY_BYTES]);
    match Base64::decode(text.as_bytes(), &mut *key) {
        Ok(decoded) if decoded.len() == KEY_BYTES => Some(key),
        _ => None,
    }
}

/// The ciphertext and signature of the newest drop of `kind` made at launch, the one with no
/// `operation`, among the drops that have both. Newest is by `created_at` read as a time, so
/// `...:00Z` and `...:00.5Z` order as the times they are, not as text; a drop whose
/// `created_at` is not an RFC 3339 time counts as older than any that is.
fn launch_drop<'a>(secrets: &'a Value, kind: &str) -> Option<(&'a str, &'a str)> {
    let text = |drop: &'a Value, field: &str| drop.get(field).and_then(Value::as_str);
    secrets
        .get("drops")?
        .as_array()?
        .iter()
        .filter(|drop| {
            text(drop, "kind") == Some(kind) && drop.get("operation").is_none_or(Value::is_null)
        })
        .filter_map(|drop| {
            let pair = (text(drop, "ciphertext")?, text(drop, "signature")?);
            Some((text(drop, "created_at").and_then(rfc3339), pair))
        })
        .max_by_key(|(created_at, _)| *created_at)
        .map(|(_, pair)| pair)
}

/// Whether `signature` is the guest key's Ed25519 signature over the drop message of
/// `ciphertext`, verified strictly (RFC 8032, and no small-order key or point).
fn verify_drop(guest: &str, cluster: u64, kind: &str, ciphertext: &str, signature: &str) -> bool {
    let mut public = [0_u8; KEY_BYTES];
    let mut signature_bytes = [0_u8; Signature::BYTE_SIZE];
    let decoded = Base64::decode(guest, &mut public).is_ok_and(|k| k.len() == KEY_BYTES)
        && Base64::decode(signature, &mut signature_bytes)
            .is_ok_and(|s| s.len() == Signature::BYTE_SIZE);
    if !decoded {
        return false;
    }
    let Ok(guest) = VerifyingKey::from_bytes(&public) else {
        return false;
    };
    let message = drop_message(cluster, kind, ciphertext);
    guest
        .verify_strict(message.as_bytes(), &Signature::from_bytes(&signature_bytes))
        .is_ok()
}

/// The message the guest signs for a drop: the hash is of the base64 text, not of the bytes
/// it decodes to.
fn drop_message(cluster: u64, kind: &str, ciphertext: &str) -> String {
    let digest = Sha256::digest(ciphertext.as_bytes());
    let hex = hex(&digest);
    format!("{DROP_DOMAIN}:{cluster}:{kind}:{hex}")
}

/// A lowercase version-4 UUID made of `random`: the version nibble set to 4 and the variant
/// bits to `10`, as RFC 9562 section 5.4 says.
pub(crate) fn uuid_v4(mut random: [u8; 16]) -> String {
    random[6] = (random[6] & 0x0f) | 0x40;
    random[8] = (random[8] & 0x3f) | 0x80;
    let hex = hex(&random);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// `bytes` in lowercase hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(out, "{byte:02x}").expect("writing to a String does not fail");
    }
    out
}

/// `value` as the backend's canonical JSON (module docs), or `None` for a number that is not
/// an integer: Python and Rust print a float differently, and nothing signed holds one. `-0`
/// is refused too, since Python reads it as `0`. Keys sort by their UTF-8 bytes, which is the
/// code-point order Python's `sort_keys` uses.
fn canonical(value: &Value) -> Option<String> {
    let mut out = String::new();
    write_canonical(value, &mut out)?;
    Some(out)
}

/// Appends `value` to `out` as [`canonical`] writes it, or `None` where it refuses.
fn write_canonical(value: &Value, out: &mut String) -> Option<()> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => {
            let text = number.to_string();
            if text == "-0" || text.contains(['.', 'e', 'E']) {
                return None;
            }
            out.push_str(&text);
        }
        Value::String(text) => python_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out)?;
            }
            out.push(']');
        }
        Value::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                python_string(key, out);
                out.push(':');
                write_canonical(&fields[key], out)?;
            }
            out.push('}');
        }
    }
    Some(())
}

/// Appends `text` as Python's `json.dumps(ensure_ascii=False)` writes a string: `"` and `\`
/// escaped, `\b \f \n \r \t` by name, every other control below U+0020 as `\u00xx` in
/// lowercase hex, and everything else, U+007F and non-ASCII included, as it is.
fn python_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests;
