//! The access key of an account: a BIP-39 phrase, the two keys it derives, and the
//! signatures of a login challenge.
//!
//! The derivation is the one the Hippius wallets use, so a phrase opens the account it opened
//! before:
//!
//! - Substrate: sr25519 from the phrase with no password, as Substrate's `from_phrase` does.
//!   The mini secret is PBKDF2-HMAC-SHA512 of the BIP-39 entropy (not of the BIP-39 seed),
//!   salted with `mnemonic`, 2048 rounds, cut to 32 bytes (as `substrate-bip39` computes it),
//!   and expanded in Ed25519 mode. The address is SS58 with the generic prefix 42.
//! - Ethereum: the BIP-39 seed with an empty passphrase, BIP-32 to `m/44'/60'/0'/0/0`
//!   (account 0), and the EIP-55 address of that key.
//!
//! The phrase is never kept: [`AccessKey::from_phrase`] reads it into the two keys, and
//! clears every buffer of its own that held the phrase, its entropy, its seed, a BIP-32
//! parent key, or a tweak. What it cannot clear: bip39 computes the seed with its own
//! PBKDF2, whose HMAC pads stay on the stack frames it returns from, and a secp256k1 key or
//! scalar passed by value leaves the copy the call made. No message quotes the phrase, and no
//! error of a library it passes through is passed on.
//!
//! A login signs a message the server chose, with the same sr25519 context a chain
//! extrinsic is signed in and the EIP-191 scheme that Sign-In with Ethereum verifiers accept,
//! so [`AccessKey::sign_login`] signs only a message that reads as a login challenge
//! ([`check_challenge`]).

use std::fmt;
use std::path::Path;

use bip39::{Language, Mnemonic};
use blake2::{Blake2b512, Digest as _};
use hmac::{Hmac, Mac as _};
use schnorrkel::{ExpansionMode, Keypair, MiniSecretKey};
use secp256k1::{Message, PublicKey, Scalar, Secp256k1, SecretKey};
use sha2::Sha512;
use sha3::Keccak256;
use unicode_normalization::UnicodeNormalization as _;
use zeroize::{Zeroize as _, Zeroizing};

use crate::config::{Guarded, NamedBy, PrivateFile};
use crate::error::{Error, Result};
use crate::kubernetes::hex;
use crate::source::Source;

/// The signing context of a Substrate sr25519 signature (`sp_core::sr25519`), which the API
/// verifies a login's `substrate_signature` against.
const SIGNING_CONTEXT: &[u8] = b"substrate";

/// The generic Substrate SS58 prefix, which the API names for `substrate_address`.
const SS58_PREFIX: u8 = 42;

/// A BIP-32 hardened index is `i + 2^31`.
const HARDENED: u32 = 0x8000_0000;

/// `m/44'/60'/0'/0/0`: BIP-44, coin type 60 (Ethereum), account 0, the external chain, the
/// first address.
const ETHEREUM_PATH: [u32; 5] = [HARDENED + 44, HARDENED + 60, HARDENED, 0, 0];

/// The two keys of an access key and their addresses. `Debug` shows the addresses only.
pub struct AccessKey {
    sr25519: Keypair,
    ethereum: EthereumKey,
    address: String,
    substrate_address: String,
}

/// The secp256k1 key, which is `Copy` in its crate: wrapped so it is never copied by
/// accident, and erased when dropped.
struct EthereumKey(SecretKey);

impl Drop for EthereumKey {
    fn drop(&mut self) {
        self.0.non_secure_erase();
    }
}

/// The longest login challenge message signed, in bytes: far above any challenge text, and
/// short enough that the message cannot carry a document to sign.
const MAX_CHALLENGE_BYTES: usize = 1024;

/// The two signatures of a login challenge, as `POST /api/auth/verify/` takes them.
#[derive(Debug)]
pub struct LoginSignatures {
    /// The EIP-191 `personal_sign` signature by the Ethereum key: `0x` and 130 hex digits,
    /// `r`, `s`, and `v` (27 or 28).
    pub signature: String,
    /// The sr25519 signature by the Substrate key, over the same bytes: `0x` and 128 hex
    /// digits.
    pub substrate_signature: String,
}

impl fmt::Debug for AccessKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessKey")
            .field("address", &self.address)
            .field("substrate_address", &self.substrate_address)
            .finish_non_exhaustive()
    }
}

impl AccessKey {
    /// The keys of the phrase in `text`, read from `source`: English BIP-39 words, 12 to 24
    /// of them, NFKD-normalized, separated by any whitespace.
    ///
    /// # Errors
    ///
    /// `usage` when `text` is not a BIP-39 phrase: the message names the source and, for an
    /// unknown word, its position, and never quotes a word.
    pub fn from_phrase(text: &str, source: &Source) -> Result<Self> {
        let mnemonic = parse(text, source)?;
        let (mut entropy, length) = mnemonic.to_entropy_array();
        let mut stretched = Zeroizing::new([0_u8; 64]);
        pbkdf2::pbkdf2_hmac::<Sha512>(&entropy[..length], b"mnemonic", 2048, &mut stretched[..]);
        entropy.zeroize();
        let sr25519 = MiniSecretKey::from_bytes(&stretched[..32])
            .expect("a mini secret is any 32 bytes")
            .expand_to_keypair(ExpansionMode::Ed25519);
        let seed = Zeroizing::new(mnemonic.to_seed_normalized(""));
        let ethereum = derive(&seed[..], &ETHEREUM_PATH);
        let substrate_address = ss58(&sr25519.public.to_bytes());
        let address = ethereum_address(&ethereum);
        Ok(Self {
            sr25519,
            ethereum,
            address,
            substrate_address,
        })
    }

    /// The EIP-55 Ethereum address, `0x` and 40 hex digits in mixed case.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The SS58 address with prefix 42.
    pub fn substrate_address(&self) -> &str {
        &self.substrate_address
    }

    /// Signs the `message` of a login challenge whose nonce is `challenge` with both keys,
    /// once `check_challenge` passes it.
    ///
    /// The sr25519 signature is over the message's raw bytes, not the `<Bytes>` wrapping a
    /// browser extension adds: swagger says the API verifies the raw bytes and accepts the
    /// wrapping only while a server setting allows it, so the raw form works either way.
    ///
    /// # Errors
    ///
    /// `internal` when the message does not read as a login challenge; nothing is signed.
    pub fn sign_login(&self, message: &str, challenge: &str) -> Result<LoginSignatures> {
        check_challenge(message, challenge).map_err(|why| {
            Error::internal(format!(
                "the login challenge {why}, so `hipc` signed nothing: a message that is not a \
                 login challenge could be a payment or a sign-in elsewhere"
            ))
        })?;
        Ok(LoginSignatures {
            signature: self.personal_sign(message.as_bytes()),
            substrate_signature: format!(
                "0x{}",
                hex(&self
                    .sr25519
                    .sign_simple(SIGNING_CONTEXT, message.as_bytes())
                    .to_bytes())
            ),
        })
    }

    /// EIP-191 `personal_sign`: an RFC 6979 ECDSA signature, low `s`, of
    /// [`personal_message_hash`], as `0x` and `r || s || v` in hex with `v` 27 or 28.
    fn personal_sign(&self, message: &[u8]) -> String {
        let digest = Message::from_digest(personal_message_hash(message));
        let signed = Secp256k1::signing_only().sign_ecdsa_recoverable(digest, &self.ethereum.0);
        let (recovery, compact) = signed.serialize_compact();
        let mut bytes = [0_u8; 65];
        bytes[..64].copy_from_slice(&compact);
        bytes[64] = legacy_v(i32::from(recovery));
        format!("0x{}", hex(&bytes))
    }
}

/// Ethereum's legacy `v`, 27 or 28, for a recovery id: its low bit is the parity of `y`. Ids
/// 2 and 3, which say `x` overflowed the curve order (a chance near 2^-127), cannot be
/// written in a legacy `v`, so their parity is kept, as Ethereum libraries do.
fn legacy_v(recovery: i32) -> u8 {
    if recovery % 2 == 0 { 27 } else { 28 }
}

/// Why `message` is not a login challenge whose nonce is `challenge`, or `Ok`.
///
/// The template of the server's message is not documented (swagger types it as a string), so
/// the rules hold what any challenge holds and a document to sign does not: the nonce the
/// answer names, inside the message; text of at most [`MAX_CHALLENGE_BYTES`] with no control
/// character, so no line break, which a Sign-In with Ethereum message needs, and no raw
/// bytes, which an extrinsic payload is; and more than 32 bytes, since Substrate signs the
/// 32-byte hash of a long payload, not the payload.
pub(crate) fn check_challenge(
    message: &str,
    challenge: &str,
) -> std::result::Result<(), &'static str> {
    if challenge.trim().is_empty() {
        return Err("has no `challenge`");
    }
    if message.len() <= 32 {
        return Err("message is 32 bytes or shorter");
    }
    if message.len() > MAX_CHALLENGE_BYTES {
        return Err("message is longer than 1024 bytes");
    }
    if message.chars().any(char::is_control) {
        return Err("message holds a control character");
    }
    if !message.contains(challenge) {
        return Err("message does not hold its `challenge`");
    }
    Ok(())
}

/// The access key phrase in the file `flag` names, held to the config file's checks: a
/// regular file of the current user, not a symlink, with no bits for the group or others, at
/// most 1 MiB, in a directory no one else can write. The bytes are read into a buffer sized
/// up front and cleared when dropped.
///
/// # Errors
///
/// `usage` when a check refuses the file, it does not exist, or it is not UTF-8 text.
pub fn read_phrase_file(path: &Path, flag: &'static str) -> Result<Zeroizing<String>> {
    let guarded = Guarded {
        what: "the access key file",
        holds: "an access key phrase",
        named_by: NamedBy::Flag(flag),
    };
    let refuse = |problem: &str| {
        Error::usage(
            format!(
                "the access key file {} {problem}",
                crate::phrase::quote_path(path)
            ),
            format!("pass `{flag}` a file that holds only the access key's words"),
        )
    };
    let Some(bytes) = PrivateFile::load(path, guarded)? else {
        // A path that is itself an access key was passed in place of a file: it is not
        // quoted, so the words go no further than the argv they are already in.
        if path.to_str().is_some_and(holds_phrase) {
            return Err(phrase_in_argv());
        }
        // Nor is a path that names no file, which may be a secret passed in its place.
        return Err(Error::usage(
            format!(
                "the access key file `{flag}` names does not exist; the path is not repeated \
                 here, in case it is the access key itself"
            ),
            format!("pass `{flag}` a file that holds only the access key's words"),
        ));
    };
    let mut bytes = Zeroizing::new(bytes);
    if std::str::from_utf8(&bytes).is_err() {
        return Err(refuse("is not UTF-8 text"));
    }
    Ok(Zeroizing::new(
        String::from_utf8(std::mem::take(&mut *bytes)).expect("checked: UTF-8"),
    ))
}

/// The fewest words in a row on the English BIP-39 list that [`holds_phrase`] takes for an
/// access key: the 12 of the shortest phrase.
const PHRASE_WORDS: usize = 12;

/// Whether `text` holds what looks like an access key: 12 or more words in a
/// row, each on the English BIP-39 list, in any letter case. A command line that does is
/// refused without being quoted ([`phrase_in_argv`]), since an error that repeats it would
/// copy the key into a log or a transcript.
pub fn holds_phrase(text: &str) -> bool {
    let mut run = 0;
    for word in text.split_whitespace() {
        let listed = Language::English
            .find_word(&word.to_ascii_lowercase())
            .is_some();
        run = if listed { run + 1 } else { 0 };
        if run >= PHRASE_WORDS {
            return true;
        }
    }
    false
}

/// The refusal of a command line that holds an access key ([`holds_phrase`]): `usage`, with
/// nothing of the command line in it.
pub fn phrase_in_argv() -> Error {
    Error::usage(
        "the command line holds what looks like an access key, so it is not repeated here; \
         treat the 12 words as exposed to anyone who can read this machine's process list or \
         shell history",
        "pass the words with `hipc auth login --access-key` at a terminal, or in a file with \
         `--access-key-file <path>`, never on the command line",
    )
}

/// The phrase in `text` as a mnemonic, or a refusal that names `source` and never quotes the
/// phrase.
fn parse(text: &str, source: &Source) -> Result<Mnemonic> {
    // ASCII is already NFKD; any other text is normalized into a buffer that is cleared when
    // dropped, which bip39's own normalization would leave behind. Its size is counted first,
    // so filling it never reallocates and frees a copy nothing clears.
    let normalized = Zeroizing::new(if text.is_ascii() {
        String::new()
    } else {
        let mut normalized = String::with_capacity(text.nfkd().map(char::len_utf8).sum());
        normalized.extend(text.nfkd());
        normalized
    });
    let words = if text.is_ascii() { text } else { &normalized };
    let from = source.suffix();
    let problem = match Mnemonic::parse_in_normalized(Language::English, words) {
        Ok(mnemonic) => return Ok(mnemonic),
        Err(_) if words.split_whitespace().next().is_none() => {
            format!("the access key phrase{from} is empty")
        }
        Err(bip39::Error::BadWordCount(count)) => format!(
            "the access key phrase{from} has {count} words, and a BIP-39 phrase has 12, 15, 18, \
             21, or 24"
        ),
        Err(bip39::Error::UnknownWord(index)) => format!(
            "word {} of the access key phrase{from} is not on the English BIP-39 word list",
            index + 1
        ),
        // The library's own words are not passed on, so none can carry the phrase.
        Err(_) => format!(
            "the access key phrase{from} fails its BIP-39 checksum: a word is wrong or out of \
             place"
        ),
    };
    let hint = match source {
        Source::File { flag, path } if path.as_os_str() == "-" => {
            format!("pipe only the access key's words to `{flag} -`")
        }
        Source::File { flag, .. } => {
            format!("leave only the access key's words in the file `{flag}` names")
        }
        Source::Argument => "type only the access key's words at the prompt".to_owned(),
        _ => "pass the file that holds the access key's words".to_owned(),
    };
    Err(Error::usage(problem, hint))
}

/// The BIP-32 private key at `path` below the master key of `seed`.
///
/// A step whose key would fall outside the curve order is not skipped to the next index, as
/// BIP-32 says, but refused: the chance is below 2^-127, and a skip would derive a key no
/// wallet expects.
fn derive(seed: &[u8], path: &[u32]) -> EthereumKey {
    let secp = Secp256k1::signing_only();
    let (key, mut chain) = split(&hmac_sha512(b"Bitcoin seed", &[seed]));
    let mut secret =
        EthereumKey(SecretKey::from_byte_array(*key).expect("BIP-32: the master key is valid"));
    drop(key);
    for &index in path {
        let index_bytes = index.to_be_bytes();
        let public = PublicKey::from_secret_key(&secp, &secret.0).serialize();
        let parent = Zeroizing::new(secret.0.secret_bytes());
        // A hardened child hashes the parent's private key, a normal one its public key.
        let data: [&[u8]; 3] = if index >= HARDENED {
            [&[0], &parent[..], &index_bytes]
        } else {
            [&public, &[], &index_bytes]
        };
        let (tweak, next_chain) = split(&hmac_sha512(&chain[..], &data));
        let tweak =
            Scalar::from_be_bytes(*tweak).expect("BIP-32: the tweak is below the curve order");
        // The parent is replaced, and so erased when the wrapper drops.
        secret = EthereumKey(
            secret
                .0
                .add_tweak(&tweak)
                .expect("BIP-32: the child key is not zero"),
        );
        chain = next_chain;
    }
    secret
}

/// HMAC-SHA512 of the concatenated `data` under `key`.
fn hmac_sha512(key: &[u8], data: &[&[u8]]) -> Zeroizing<[u8; 64]> {
    let mut mac = Hmac::<Sha512>::new_from_slice(key).expect("HMAC takes a key of any length");
    for part in data {
        mac.update(part);
    }
    let mut out = Zeroizing::new([0_u8; 64]);
    let mut tag = mac.finalize().into_bytes();
    out.copy_from_slice(&tag);
    tag.zeroize();
    out
}

/// The left and right halves of a BIP-32 HMAC output: the key or tweak, and the chain code.
fn split(bytes: &[u8; 64]) -> (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>) {
    let mut left = Zeroizing::new([0_u8; 32]);
    let mut right = Zeroizing::new([0_u8; 32]);
    left.copy_from_slice(&bytes[..32]);
    right.copy_from_slice(&bytes[32..]);
    (left, right)
}

/// The EIP-191 hash of `message`: Keccak-256 of `"\x19Ethereum Signed Message:\n"`, the
/// message's length in decimal, and the message.
pub(crate) fn personal_message_hash(message: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(b"\x19Ethereum Signed Message:\n");
    hasher.update(message.len().to_string().as_bytes());
    hasher.update(message);
    hasher.finalize().into()
}

/// The EIP-55 address of `key`: the last 20 bytes of the Keccak-256 of its uncompressed
/// public key without the `04` tag, each hex letter in upper case where the same nibble of
/// the Keccak-256 of the lowercase hex is 8 or more.
fn ethereum_address(key: &EthereumKey) -> String {
    let public = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key.0);
    ethereum_address_of(&public)
}

/// [`ethereum_address`] of a public key.
pub(crate) fn ethereum_address_of(public: &PublicKey) -> String {
    let digest = Keccak256::digest(&public.serialize_uncompressed()[1..]);
    let lower = hex(&digest[12..]);
    let check = Keccak256::digest(lower.as_bytes());
    let mut address = String::from("0x");
    for (i, c) in lower.chars().enumerate() {
        let nibble = (check[i / 2] >> if i % 2 == 0 { 4 } else { 0 }) & 0x0f;
        address.push(if nibble >= 8 {
            c.to_ascii_uppercase()
        } else {
            c
        });
    }
    address
}

/// The SS58 address of a 32-byte public key with [`SS58_PREFIX`]: base58 of the prefix, the
/// key, and the first two bytes of the BLAKE2b-512 of `SS58PRE`, the prefix, and the key.
fn ss58(public: &[u8; 32]) -> String {
    let mut payload = vec![SS58_PREFIX];
    payload.extend_from_slice(public);
    let checksum = Blake2b512::new()
        .chain_update(b"SS58PRE")
        .chain_update(&payload)
        .finalize();
    payload.extend_from_slice(&checksum[..2]);
    base58(&payload)
}

/// `bytes` in base58 with the Bitcoin alphabet: one `1` for each leading zero byte, then the
/// rest as a big-endian number in base 58.
fn base58(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let zeros = bytes.iter().take_while(|&&b| b == 0).count();
    // Base-58 digits, least significant first.
    let mut digits: Vec<u8> = Vec::new();
    for &byte in &bytes[zeros..] {
        let mut carry = u32::from(byte);
        for digit in &mut digits {
            carry += u32::from(*digit) << 8;
            *digit = u8::try_from(carry % 58).expect("a remainder of 58 fits a byte");
            carry /= 58;
        }
        while carry > 0 {
            digits.push(u8::try_from(carry % 58).expect("a remainder of 58 fits a byte"));
            carry /= 58;
        }
    }
    let mut text = "1".repeat(zeros);
    text.extend(
        digits
            .iter()
            .rev()
            .map(|&d| char::from(ALPHABET[usize::from(d)])),
    );
    text
}

/// A new random idempotency key: a lowercase version-4 UUID.
///
/// # Errors
///
/// `internal` when the system gives no random bytes.
pub fn new_idempotency_key() -> Result<String> {
    let mut random = [0_u8; 16];
    getrandom::getrandom(&mut random).map_err(no_random_bytes)?;
    Ok(crate::kubernetes::uuid_v4(random))
}

/// The refusal when the system gives no random bytes for a key.
fn no_random_bytes(error: getrandom::Error) -> Error {
    Error::internal(format!(
        "cannot read random bytes for an idempotency key: the system says {}",
        crate::phrase::quote(error)
    ))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use secp256k1::ecdsa::{RecoverableSignature, RecoveryId};

    use super::*;

    /// Foundry's, Hardhat's, and Anvil's default phrase.
    const TEST_PHRASE: &str = "test test test test test test test test test test test junk";
    /// What [`TEST_PHRASE`]'s Ethereum key signs for "Sign in to Hippius with challenge
    /// e2e-challenge".
    const PINNED_SIGNATURE: &str = concat!(
        "0xe1bfb26120b8c1290a3f34fae6dc46916386c599927a19b1c0bb3cd8a76c1dc4",
        "2bcbf46dd8c7440965581db7c06666879abba801ad12472402f728c2a71a417b1b",
    );
    /// Substrate's development phrase, `DEV_PHRASE` in `sp-core`.
    const DEV_PHRASE: &str =
        "bottom drive obey lake curtain smoke basket hold race lonely fit walk";

    fn file() -> Source {
        Source::File {
            flag: "--access-key-file",
            path: PathBuf::from("phrase.txt"),
        }
    }

    fn key(phrase: &str) -> AccessKey {
        AccessKey::from_phrase(phrase, &file()).unwrap()
    }

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    /// The Ethereum address is Foundry's default account 0, which every Ethereum tool
    /// derives from this phrase. The SS58 address is the one Hippius wallets derive from it;
    /// a change here would open a different account for every existing phrase.
    #[test]
    fn the_test_phrase_derives_the_known_addresses() {
        let key = key(TEST_PHRASE);
        assert_eq!(key.address(), "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        assert_eq!(
            key.substrate_address(),
            "5GmS1wtCfR4tK5SSgnZbVT4kYw5W8NmxmijcsxCQE6oLW6A8"
        );
    }

    /// `subkey inspect` of the bare development phrase (no `//Alice`, which is a hard
    /// derivation below it), as Substrate's documentation prints it: public key
    /// `0x46ebddef8cd9bb167dc30878d7113b7e168e6f0646beffd77d69d39bad76b47a`.
    #[test]
    fn the_substrate_dev_phrase_derives_the_subkey_address() {
        let key = key(DEV_PHRASE);
        assert_eq!(
            hex(&key.sr25519.public.to_bytes()),
            "46ebddef8cd9bb167dc30878d7113b7e168e6f0646beffd77d69d39bad76b47a"
        );
        assert_eq!(
            key.substrate_address(),
            "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV"
        );
    }

    /// Moonbeam's development account Alith is `m/44'/60'/0'/0/0` of the same phrase, and its
    /// documentation publishes the key and the address.
    #[test]
    fn the_substrate_dev_phrase_derives_alith_on_ethereum() {
        let key = key(DEV_PHRASE);
        assert_eq!(
            hex(&key.ethereum.0.secret_bytes()),
            "5fb92d6e98884f76de468fa3f6278f8807c48bebc13595d45af5bdc4da702133"
        );
        assert_eq!(key.address(), "0xf24FF3a9CF04c71Dbc94D0b566f7A27B94566cac");
    }

    /// BIP-32 test vector 1, chain `m/0H/1/2H/2/1000000000`, each private key in turn.
    #[test]
    fn bip32_matches_test_vector_1() {
        let seed = unhex("000102030405060708090a0b0c0d0e0f");
        let expected = [
            (
                &[][..],
                "e8f32e723decf4051aefac8e2c93c9c5b214313817cdb01a1494b917c8436b35",
            ),
            (
                &[HARDENED][..],
                "edb2e14f9ee77d26dd93b4ecede8d16ed408ce149b6cd80b0715a2d911a0afea",
            ),
            (
                &[HARDENED, 1][..],
                "3c6cb8d0f6a264c91ea8b5030fadaa8e538b020f0a387421a12de9319dc93368",
            ),
            (
                &[HARDENED, 1, HARDENED + 2][..],
                "cbce0d719ecf7431d88e6a89fa1483e02e35092af60c042b1df2ff59fa424dca",
            ),
            (
                &[HARDENED, 1, HARDENED + 2, 2][..],
                "0f479245fb19a38a1954c5c7c0ebab2f9bdfd96a17563ef28a6a4b1a2a764ef4",
            ),
            (
                &[HARDENED, 1, HARDENED + 2, 2, 1_000_000_000][..],
                "471b76e389e528d6de6d816857e012c5455051cad6660850e58372a6c3e6e7c8",
            ),
        ];
        for (path, secret) in expected {
            assert_eq!(
                hex(&derive(&seed, path).0.secret_bytes()),
                secret,
                "{path:?}"
            );
        }
    }

    /// Recovers the signer's address from a `personal_sign` signature, as the API checks it.
    fn recovered_address(message: &[u8], signature: &str) -> String {
        let bytes = unhex(signature.strip_prefix("0x").unwrap());
        assert_eq!(bytes.len(), 65);
        let recovery = RecoveryId::try_from(i32::from(bytes[64]) - 27).unwrap();
        let signature = RecoverableSignature::from_compact(&bytes[..64], recovery).unwrap();
        let public = Secp256k1::verification_only()
            .recover_ecdsa(
                Message::from_digest(personal_message_hash(message)),
                &signature,
            )
            .unwrap();
        ethereum_address_of(&public)
    }

    #[test]
    fn the_ethereum_signature_recovers_to_the_address() {
        let key = key(TEST_PHRASE);
        for message in [
            "Sign in to Hippius with challenge 1f2e3d4c5b6a",
            "ünïcode ✓ challenge 1f2e3d4c5b6a, signed in to Hippius",
        ] {
            let signed = key.sign_login(message, "1f2e3d4c5b6a").unwrap();
            assert_eq!(signed.signature.len(), 132);
            assert_eq!(
                recovered_address(message.as_bytes(), &signed.signature),
                key.address()
            );
            assert_ne!(
                recovered_address(b"another message", &signed.signature),
                key.address()
            );
        }
    }

    /// RFC 6979 makes the ECDSA signature deterministic, and libsecp256k1 gives the low `s`
    /// that Ethereum requires (EIP-2).
    #[test]
    fn the_ethereum_signature_is_deterministic_and_low_s() {
        let key = key(TEST_PHRASE);
        let message = "Sign in to Hippius with challenge e2e-challenge";
        let first = key.sign_login(message, "e2e-challenge").unwrap().signature;
        assert_eq!(
            first,
            key.sign_login(message, "e2e-challenge").unwrap().signature
        );
        // A regression anchor: the bytes this key and message gave when the signer was added.
        assert_eq!(first, PINNED_SIGNATURE);
        let s = unhex(&first[66..130]);
        // Low s: below half the curve order, whose top byte is 0x7f.
        assert!(s[0] <= 0x7f, "{first}");
        assert!(first.ends_with("1b") || first.ends_with("1c"), "{first}");
    }

    #[test]
    fn the_eip191_hash_prefixes_the_length() {
        // keccak256("\x19Ethereum Signed Message:\n11Hello World"), as every EIP-191 library
        // computes it.
        assert_eq!(
            hex(&personal_message_hash(b"Hello World")),
            "a1de988600a42c4b4ab089b619297c17d53cffae5d5120d82d8a92d0bb3b78f2"
        );
    }

    #[test]
    fn the_sr25519_signature_verifies_in_the_substrate_context() {
        let key = key(TEST_PHRASE);
        let message = "Sign in to Hippius with challenge 1f2e3d4c5b6a";
        let signed = key.sign_login(message, "1f2e3d4c5b6a").unwrap();
        let hex_digits = signed.substrate_signature.strip_prefix("0x").unwrap();
        assert_eq!(hex_digits.len(), 128);
        let signature = schnorrkel::Signature::from_bytes(&unhex(hex_digits)).unwrap();
        let public = &key.sr25519.public;
        assert!(
            public
                .verify_simple(SIGNING_CONTEXT, message.as_bytes(), &signature)
                .is_ok()
        );
        assert!(
            public
                .verify_simple(SIGNING_CONTEXT, b"another message", &signature)
                .is_err()
        );
        assert!(
            public
                .verify_simple(b"other", message.as_bytes(), &signature)
                .is_err()
        );
        // A raw-bytes signature is not one of the `<Bytes>`-wrapped message.
        let wrapped = format!("<Bytes>{message}</Bytes>");
        assert!(
            public
                .verify_simple(SIGNING_CONTEXT, wrapped.as_bytes(), &signature)
                .is_err()
        );
    }

    #[test]
    fn whitespace_and_nfkd_forms_derive_the_same_keys() {
        let plain = key(TEST_PHRASE);
        let spaced = key(&format!("  {}\n", TEST_PHRASE.replace(' ', " \t\n ")));
        assert_eq!(spaced.address(), plain.address());
        assert_eq!(spaced.substrate_address(), plain.substrate_address());
        // An ideographic space (U+3000) is NFKD-normalized to a plain space, and so is a
        // no-break space.
        let wide = key(&TEST_PHRASE.replace(' ', "\u{3000}"));
        assert_eq!(wide.substrate_address(), plain.substrate_address());
        let nbsp = key(&DEV_PHRASE.replace(' ', "\u{a0}"));
        assert_eq!(nbsp.address(), key(DEV_PHRASE).address());
    }

    #[test]
    fn a_bad_phrase_is_refused_without_quoting_it() {
        let cases = [
            ("", "the access key phrase from `phrase.txt` is empty"),
            (" \n\t", "the access key phrase from `phrase.txt` is empty"),
            (
                "test test test",
                "the access key phrase from `phrase.txt` has 3 words, and a BIP-39 phrase has \
                 12, 15, 18, 21, or 24",
            ),
            (
                "test test test test zzsentinel test test test test test test junk",
                "word 5 of the access key phrase from `phrase.txt` is not on the English \
                 BIP-39 word list",
            ),
            (
                "Test test test test test test test test test test test junk",
                "word 1 of the access key phrase from `phrase.txt` is not on the English \
                 BIP-39 word list",
            ),
            (
                "test test test test test test test test test test test test",
                "the access key phrase from `phrase.txt` fails its BIP-39 checksum: a word is \
                 wrong or out of place",
            ),
            (
                "tést test test test test test test test test test test junk",
                "word 1 of the access key phrase from `phrase.txt` is not on the English \
                 BIP-39 word list",
            ),
        ];
        for (phrase, message) in cases {
            let error = AccessKey::from_phrase(phrase, &file()).unwrap_err();
            assert_eq!(error.message(), message, "{phrase:?}");
            assert_eq!(
                error.hint(),
                Some("leave only the access key's words in the file `--access-key-file` names")
            );
            assert!(!error.message().contains("zzsentinel"));
        }
        let stdin = Source::File {
            flag: "--access-key-file",
            path: PathBuf::from("-"),
        };
        let error = AccessKey::from_phrase("x", &stdin).unwrap_err();
        assert_eq!(
            error.hint(),
            Some("pipe only the access key's words to `--access-key-file -`")
        );
        let error = AccessKey::from_phrase("x", &Source::Default).unwrap_err();
        assert_eq!(
            error.hint(),
            Some("pass the file that holds the access key's words")
        );
    }

    #[test]
    fn debug_shows_the_addresses_only() {
        let key = key(TEST_PHRASE);
        let debug = format!("{key:?}");
        assert!(debug.contains("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"));
        assert!(!debug.contains("sr25519") && !debug.contains("ethereum:"));
        let signatures = key
            .sign_login(
                "Sign in to Hippius with challenge 1f2e3d4c5b6a",
                "1f2e3d4c5b6a",
            )
            .unwrap();
        assert!(format!("{signatures:?}").contains(&signatures.signature));
    }

    #[test]
    fn legacy_v_keeps_the_parity_of_every_recovery_id() {
        assert_eq!([0, 1, 2, 3].map(legacy_v), [27, 28, 27, 28]);
    }

    #[test]
    fn only_a_login_challenge_is_signed() {
        let key = key(TEST_PHRASE);
        let nonce = "8f2c41d09a7b";
        let ok = format!("Sign in to Hippius with challenge {nonce}");
        assert!(key.sign_login(&ok, nonce).is_ok());
        let long = format!("{nonce} {}", "a".repeat(1024));
        let siwe = format!(
            "example.com wants you to sign in with your Ethereum account:\n{}\n\nNonce: {nonce}",
            key.address()
        );
        let cases = [
            (ok.as_str(), "", "has no `challenge`"),
            (ok.as_str(), "  ", "has no `challenge`"),
            (
                ok.as_str(),
                "another-nonce",
                "message does not hold its `challenge`",
            ),
            (
                "8f2c41d09a7b, 32 bytes in all!!",
                nonce,
                "message is 32 bytes or shorter",
            ),
            (long.as_str(), nonce, "message is longer than 1024 bytes"),
            (siwe.as_str(), nonce, "message holds a control character"),
            (
                "Sign in to Hippius 8f2c41d09a7b\u{1b}[2J",
                nonce,
                "message holds a control character",
            ),
        ];
        for (message, challenge, why) in cases {
            assert_eq!(check_challenge(message, challenge), Err(why), "{message:?}");
            let error = key.sign_login(message, challenge).unwrap_err();
            assert_eq!(error.code(), crate::error::Code::Internal);
            assert!(error.message().contains(why), "{}", error.message());
        }
        // Exactly 33 and exactly 1024 bytes are signed.
        let short = format!("{nonce}{}", "x".repeat(33 - nonce.len()));
        assert!(check_challenge(&short, nonce).is_ok());
        let edge = format!("{nonce}{}", "x".repeat(1024 - nonce.len()));
        assert!(check_challenge(&edge, nonce).is_ok());
    }

    #[test]
    fn base58_keeps_leading_zeros() {
        assert_eq!(base58(&[]), "");
        assert_eq!(base58(&[0, 0, 1]), "112");
        assert_eq!(base58(b"hello world"), "StV1DL6CwTryKyV");
        assert_eq!(base58(&[0xff; 4]), "7YXq9G");
    }

    #[test]
    fn no_random_bytes_is_internal() {
        let error = no_random_bytes(getrandom::Error::UNSUPPORTED);
        assert_eq!(error.code(), crate::error::Code::Internal);
        let message = error.message();
        assert!(message.starts_with("cannot read random bytes for an idempotency key: the system"));
    }

    #[test]
    fn idempotency_keys_are_new_v4_uuids() {
        let first = new_idempotency_key().unwrap();
        assert_eq!(first.len(), 36);
        assert_eq!(&first[14..15], "4");
        assert_ne!(first, new_idempotency_key().unwrap());
    }
}
