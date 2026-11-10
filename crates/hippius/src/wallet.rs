//! A Bittensor hotkey's sr25519 key, read from its BIP-39 phrase, and the signature of a gamma
//! link challenge; the guards that spot a seed phrase on the command line or in a support
//! ticket; and the idempotency key of a hAlpha buy.
//!
//! The key is derived as Substrate's `from_phrase` derives it with no password, the way
//! `btcli` makes a hotkey: the mini secret is PBKDF2-HMAC-SHA512 of the BIP-39 entropy (not
//! of the BIP-39 seed), salted with `mnemonic`, 2048 rounds, cut to 32 bytes (as
//! `substrate-bip39` computes it), and expanded in Ed25519 mode. The address is SS58 with the
//! generic prefix 42.
//!
//! The phrase is never kept: [`Hotkey::from_phrase`] reads it into the key, and clears every
//! buffer of its own that held the phrase, its entropy, or the stretched secret. No message
//! quotes the phrase, and no error of a library it passes through is passed on.
//!
//! The key signs with the same sr25519 context a chain extrinsic is signed in, so
//! [`Hotkey::sign_gamma_link`] signs only a message that reads as the link challenge of its own
//! hotkey ([`check_gamma_link`]).

use std::fmt;
use std::path::Path;

use bip39::{Language, Mnemonic};
use blake2::{Blake2b512, Digest as _};
use schnorrkel::{ExpansionMode, Keypair, MiniSecretKey};
use sha2::Sha512;
use unicode_normalization::UnicodeNormalization as _;
use zeroize::{Zeroize as _, Zeroizing};

use crate::encode::hex;
use crate::error::{Error, Result};
use crate::private_file::{Guarded, NamedBy, PrivateFile};
use crate::source::Source;

/// The signing context of a Substrate sr25519 signature (`sp_core::sr25519`), which the API
/// verifies a gamma link's signature against.
const SIGNING_CONTEXT: &[u8] = b"substrate";

/// The generic Substrate SS58 prefix, which the API names hotkeys in.
const SS58_PREFIX: u8 = 42;

/// The sr25519 key of a Bittensor hotkey and its SS58 address. `Debug` shows the address only.
pub struct Hotkey {
    sr25519: Keypair,
    ss58_address: String,
}

/// The longest challenge message signed, in bytes: far above any challenge text, and short
/// enough that the message cannot carry a document to sign.
const MAX_CHALLENGE_BYTES: usize = 1024;

impl fmt::Debug for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hotkey")
            .field("ss58_address", &self.ss58_address)
            .finish_non_exhaustive()
    }
}

impl Hotkey {
    /// The key of the hotkey phrase in `text`, read from `source`: English BIP-39 words, 12 to
    /// 24 of them, NFKD-normalized, separated by any whitespace. Its SS58 address is the
    /// hotkey.
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
        let ss58_address = ss58(&sr25519.public.to_bytes());
        Ok(Self {
            sr25519,
            ss58_address,
        })
    }

    /// The SS58 address with prefix 42: the hotkey.
    pub fn ss58_address(&self) -> &str {
        &self.ss58_address
    }

    /// Signs the `message` of a gamma hotkey-link challenge whose nonce is `nonce`, once the
    /// message reads as a link challenge of its own hotkey: `0x` and 128 hex digits.
    ///
    /// The message must name this key's hotkey: the API links the hotkey the challenge was
    /// issued for, and a message that names another is not one this key was asked to sign.
    /// The signature is over the message's raw bytes, which the API verifies; it is not a
    /// signature of the `<Bytes>` wrapping a browser extension adds.
    ///
    /// # Errors
    ///
    /// `internal` when the message does not read as a gamma link challenge for this hotkey;
    /// nothing is signed.
    pub fn sign_gamma_link(&self, message: &str, nonce: &str) -> Result<String> {
        check_gamma_link(message, nonce, &self.ss58_address).map_err(|why| {
            Error::internal(format!(
                "the gamma link challenge {why}, so `hipc` signed nothing: a message that is not \
                 a link challenge for this hotkey could be a payment or a sign-in elsewhere"
            ))
        })?;
        Ok(format!(
            "0x{}",
            hex(&self
                .sr25519
                .sign_simple(SIGNING_CONTEXT, message.as_bytes())
                .to_bytes())
        ))
    }
}

/// Why `message` is not a challenge whose nonce is `challenge`, or `Ok`.
///
/// The rules hold what any challenge holds and a document to sign does not: the nonce the
/// answer names, inside the message; text of at most [`MAX_CHALLENGE_BYTES`] with no control
/// character or Unicode line or paragraph separator, so no line break, which a Sign-In with
/// Ethereum message needs, and no raw bytes, which an extrinsic payload is; and more than 32
/// bytes, since Substrate signs the 32-byte hash of a long payload, not the payload.
fn check_challenge(message: &str, challenge: &str) -> std::result::Result<(), &'static str> {
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
    // U+2028 and U+2029 are not control characters, but a verifier that splits on Unicode
    // line breaks reads them as one, as it would a line feed.
    if message.contains(['\u{2028}', '\u{2029}']) {
        return Err("message holds a line or paragraph separator");
    }
    if !message.contains(challenge) {
        return Err("message does not hold its `challenge`");
    }
    Ok(())
}

/// How every gamma hotkey-link challenge message starts: the API's template is
/// `Hippius: pay with the gamma of <hotkey> for <account> nonce <nonce>`.
const GAMMA_LINK_PREFIX: &str = "Hippius: pay with the gamma of ";

/// Why `message` is not a gamma link challenge for `hotkey` whose nonce is `nonce`, or `Ok`:
/// the rules of [`check_challenge`], and the API's template exactly, with `hotkey`, an account
/// id of letters, digits, `-`, and `_`, and `nonce` in their places and nothing after, so the
/// key signs only the link of its own hotkey and no other text.
pub(crate) fn check_gamma_link(
    message: &str,
    nonce: &str,
    hotkey: &str,
) -> std::result::Result<(), &'static str> {
    check_challenge(message, nonce)?;
    let account_id = |id: &str| {
        !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    let matches = message
        .strip_prefix(GAMMA_LINK_PREFIX)
        .and_then(|rest| rest.strip_prefix(hotkey))
        .and_then(|rest| rest.strip_prefix(" for "))
        .and_then(|rest| rest.split_once(" nonce "))
        .is_some_and(|(account, rest)| account_id(account) && rest == nonce);
    if matches {
        Ok(())
    } else {
        Err("message is not the link of this hotkey")
    }
}

/// The hotkey phrase in the file `flag` names, held to the config file's checks: a regular
/// file of the current user, not a symlink, with no bits for the group or others, at most 1
/// MiB, in a directory no one else can write. It holds the BIP-39 words a Bittensor hotkey was
/// made from, such as `btcli` prints at its creation. The bytes are read into a buffer sized up
/// front and cleared when dropped. Messages name the hotkey, never the words.
///
/// # Errors
///
/// `usage` when a check refuses the file, it does not exist, or it is not UTF-8 text.
pub fn read_hotkey_file(path: &Path, flag: &'static str) -> Result<Zeroizing<String>> {
    let what = "the hotkey file";
    let guarded = Guarded {
        what,
        holds: "a hotkey phrase",
        named_by: NamedBy::Flag(flag),
    };
    let Some(bytes) = PrivateFile::load(path, guarded)? else {
        // A path that is itself a phrase was passed in place of a file: it is not quoted, so
        // the words go no further than the argv they are already in.
        if path.to_str().is_some_and(holds_phrase) {
            return Err(phrase_in_argv());
        }
        // Nor is a path that names no file, which may be a secret passed in its place.
        return Err(Error::usage(
            format!(
                "{what} `{flag}` names does not exist; the path is not repeated here, in case \
                 it is the hotkey itself"
            ),
            format!("pass `{flag}` a file that holds only the hotkey's words"),
        ));
    };
    // A path that is itself a phrase is named by its flag, as a missing one is.
    let named = if path.to_str().is_some_and(holds_phrase) {
        format!("`{flag}` names")
    } else {
        crate::phrase::quote_path(path)
    };
    String::from_utf8(bytes)
        .map(Zeroizing::new)
        .map_err(|error| {
            // The bytes the conversion hands back are cleared too.
            drop(Zeroizing::new(error.into_bytes()));
            Error::usage(
                format!("{what} {named} is not UTF-8 text"),
                format!("pass `{flag}` a file that holds only the hotkey's words"),
            )
        })
}

/// The fewest words in a row on the English BIP-39 list that [`holds_phrase`] takes for a
/// seed phrase: the 12 of the shortest phrase.
const PHRASE_WORDS: usize = 12;

/// Whether `text` holds what looks like a seed phrase, such as a hotkey's words or an
/// account's access key: 12 or more words in a row, each on the English BIP-39 list, in any
/// letter case. `hipc` refuses a command line that does, on every run and before it parses,
/// without quoting it ([`phrase_in_argv`]), since an error that repeats it would copy the
/// words into a log or a transcript.
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

/// The characters that join the parts of a word without a visible mark: the soft hyphen, the
/// zero-width space, joiners, the word joiner, and the byte order mark. Dropped rather than
/// read as a space, so they cannot split a word of a seed phrase.
pub const INVISIBLE_JOINS: [char; 6] = [
    '\u{ad}', '\u{200b}', '\u{200c}', '\u{200d}', '\u{2060}', '\u{feff}',
];

/// The lengths of a BIP-39 phrase, in words.
const PHRASE_LENGTHS: [usize; 5] = [12, 15, 18, 21, 24];

/// Whether `text` holds a seed phrase, such as an account's access key, that a person must
/// not read, as in a support ticket:
/// 12, 15, 18, 21, or 24 words in a row on the English BIP-39 list whose checksum holds, after
/// NFKD normalization, in any letter case, with anything that is not a letter read as a space,
/// so a numbered or comma-separated list of the words is found.
///
/// The checksum tells a key from prose: everyday words (`please`, `help`, `account`, `error`)
/// are on the list, and twelve of them in a row are common in a message, but only one such
/// run in 16 has a valid checksum. A key with a mistyped word fails it too, so this guards
/// against a pasted key, not a damaged one. Every copy of the text is cleared when dropped.
pub fn holds_checked_phrase(text: &str) -> bool {
    let mut words = Zeroizing::new(String::with_capacity(text.nfkd().map(char::len_utf8).sum()));
    words.extend(
        text.nfkd()
            .filter(|c| !INVISIBLE_JOINS.contains(c))
            .map(|c| {
                if c.is_alphabetic() {
                    c.to_ascii_lowercase()
                } else {
                    ' '
                }
            }),
    );
    let words: Vec<&str> = words.split_whitespace().collect();
    let listed = |word: &&&str| Language::English.find_word(word).is_some();
    let mut at = 0;
    while at < words.len() {
        let run = words[at..].iter().take_while(listed).count();
        let checked = PHRASE_LENGTHS.iter().any(|&length| {
            words[at..at + run].windows(length).any(|window| {
                let candidate = Zeroizing::new(window.join(" "));
                Mnemonic::parse_in_normalized(Language::English, &candidate).is_ok()
            })
        });
        if checked {
            return true;
        }
        at += run.max(1);
    }
    false
}

/// The refusal of a command line that holds a seed phrase ([`holds_phrase`]): `usage`, with
/// nothing of the command line in it. `hipc` takes no account access key, and the only input
/// that takes such words is the hotkey file of `hipc billing gamma hotkeys create`, so the
/// hint keeps the two apart: an access key put in the hotkey file would link the account's
/// own key as a hotkey.
pub fn phrase_in_argv() -> Error {
    Error::usage(
        "the command line holds what looks like a seed phrase, so it is not repeated here; \
         treat the words as exposed to anyone who can read this machine's process list or shell \
         history",
        "keep seed phrases off the command line: sign in with a console token at `hipc auth \
         login`, and pass a Bittensor hotkey's words only in the file `--hotkey-file` names",
    )
}

/// The hotkey phrase in `text` as a mnemonic, or a refusal that names `source` and never
/// quotes the phrase.
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
            format!("the hotkey phrase{from} is empty")
        }
        Err(bip39::Error::BadWordCount(count)) => format!(
            "the hotkey phrase{from} has {count} words, and a BIP-39 phrase has 12, 15, 18, \
             21, or 24"
        ),
        Err(bip39::Error::UnknownWord(index)) => format!(
            "word {} of the hotkey phrase{from} is not on the English BIP-39 word list",
            index + 1
        ),
        // The library's own words are not passed on, so none can carry the phrase.
        Err(_) => format!(
            "the hotkey phrase{from} fails its BIP-39 checksum: a word is wrong or out of \
             place"
        ),
    };
    let hint = match source {
        Source::File { flag, path } if path.as_os_str() == "-" => {
            format!("pipe only the hotkey's words to `{flag} -`")
        }
        Source::File { flag, .. } => {
            format!("leave only the hotkey's words in the file `{flag}` names")
        }
        Source::Argument
        | Source::Flag(_)
        | Source::Variable(_)
        | Source::Profile { .. }
        | Source::Default => "pass the file that holds the hotkey's words".to_owned(),
    };
    Err(Error::usage(problem, hint))
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

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    use std::path::PathBuf;

    use super::*;

    /// Foundry's, Hardhat's, and Anvil's default phrase.
    const TEST_PHRASE: &str = "test test test test test test test test test test test junk";
    /// Substrate's development phrase, `DEV_PHRASE` in `sp-core`.
    const DEV_PHRASE: &str =
        "bottom drive obey lake curtain smoke basket hold race lonely fit walk";

    fn file() -> Source {
        Source::File {
            flag: "--hotkey-file",
            path: PathBuf::from("hotkey.txt"),
        }
    }

    fn key(phrase: &str) -> Hotkey {
        Hotkey::from_phrase(phrase, &file()).unwrap()
    }

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    /// A hotkey file path that holds a phrase is never quoted: a missing one is refused as a
    /// phrase on the command line, and one that exists is named by its flag.
    #[test]
    fn a_hotkey_path_that_holds_a_phrase_is_not_quoted() {
        let dir = std::env::temp_dir().join(format!("hipc-hotkey-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&dir)
            .unwrap();
        let named = dir.join(format!("x {DEV_PHRASE}"));
        let missing = read_hotkey_file(&named, "--hotkey-file").unwrap_err();
        assert_eq!(missing.message(), phrase_in_argv().message());
        std::fs::write(&named, b"\xff\xfe").unwrap();
        std::fs::set_permissions(&named, std::fs::Permissions::from_mode(0o600)).unwrap();
        let error = read_hotkey_file(&named, "--hotkey-file").unwrap_err();
        assert_eq!(
            error.message(),
            "the hotkey file `--hotkey-file` names is not UTF-8 text"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The SS58 address is the one Substrate tools and Hippius wallets derive from this
    /// phrase; a change here would read a different hotkey from every existing phrase.
    #[test]
    fn the_test_phrase_derives_the_known_address() {
        assert_eq!(
            key(TEST_PHRASE).ss58_address(),
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
            key.ss58_address(),
            "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV"
        );
    }

    #[test]
    fn whitespace_and_nfkd_forms_derive_the_same_key() {
        let plain = key(TEST_PHRASE);
        let spaced = key(&format!("  {}\n", TEST_PHRASE.replace(' ', " \t\n ")));
        assert_eq!(spaced.ss58_address(), plain.ss58_address());
        // An ideographic space (U+3000) is NFKD-normalized to a plain space, and so is a
        // no-break space.
        let wide = key(&TEST_PHRASE.replace(' ', "\u{3000}"));
        assert_eq!(wide.ss58_address(), plain.ss58_address());
        let nbsp = key(&DEV_PHRASE.replace(' ', "\u{a0}"));
        assert_eq!(nbsp.ss58_address(), key(DEV_PHRASE).ss58_address());
    }

    fn gamma_message(hotkey: &str) -> String {
        format!("Hippius: pay with the gamma of {hotkey} for 42 nonce 4f1c2b3a49")
    }

    #[test]
    fn the_gamma_link_signature_verifies_over_the_raw_message() {
        let key = key(TEST_PHRASE);
        let message = gamma_message(key.ss58_address());
        let signed = key.sign_gamma_link(&message, "4f1c2b3a49").unwrap();
        let hex_digits = signed.strip_prefix("0x").unwrap();
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
    fn a_gamma_link_of_another_hotkey_or_another_text_is_not_signed() {
        let other = key(DEV_PHRASE);
        let key = key(TEST_PHRASE);
        for (message, nonce) in [
            // Another hotkey's link.
            (gamma_message(other.ss58_address()), "4f1c2b3a49"),
            // This hotkey's address as the start of a longer one.
            (
                format!(
                    "Hippius: pay with the gamma of {}x for 42 nonce 4f1c2b3a49",
                    key.ss58_address()
                ),
                "4f1c2b3a49",
            ),
            // Not a gamma link at all, though it names the hotkey and the nonce.
            (
                format!(
                    "Sign in to Hippius as {} with challenge 4f1c2b3a49",
                    key.ss58_address()
                ),
                "4f1c2b3a49",
            ),
            // The nonce the answer names is not in the message.
            (gamma_message(key.ss58_address()), "ffffffff"),
            // The template with more text after the nonce.
            (
                format!("{} and pay 100 TAO", gamma_message(key.ss58_address())),
                "4f1c2b3a49",
            ),
            // The nonce elsewhere than at the end, and an account id with a space.
            (
                format!(
                    "Hippius: pay with the gamma of {} for a b nonce 4f1c2b3a49",
                    key.ss58_address()
                ),
                "4f1c2b3a49",
            ),
        ] {
            let error = key.sign_gamma_link(&message, nonce).unwrap_err();
            assert_eq!(error.code(), crate::error::Code::Internal, "{message}");
            assert!(error.to_string().contains("signed nothing"), "{error}");
        }
    }

    #[test]
    fn a_bad_phrase_is_refused_without_quoting_it() {
        let cases = [
            ("", "the hotkey phrase from `hotkey.txt` is empty"),
            (" \n\t", "the hotkey phrase from `hotkey.txt` is empty"),
            (
                "test test test",
                "the hotkey phrase from `hotkey.txt` has 3 words, and a BIP-39 phrase has 12, \
                 15, 18, 21, or 24",
            ),
            (
                "test test test test zzsentinel test test test test test test junk",
                "word 5 of the hotkey phrase from `hotkey.txt` is not on the English BIP-39 \
                 word list",
            ),
            (
                "Test test test test test test test test test test test junk",
                "word 1 of the hotkey phrase from `hotkey.txt` is not on the English BIP-39 \
                 word list",
            ),
            (
                "test test test test test test test test test test test test",
                "the hotkey phrase from `hotkey.txt` fails its BIP-39 checksum: a word is wrong \
                 or out of place",
            ),
            (
                "tést test test test test test test test test test test junk",
                "word 1 of the hotkey phrase from `hotkey.txt` is not on the English BIP-39 \
                 word list",
            ),
        ];
        for (phrase, message) in cases {
            let error = Hotkey::from_phrase(phrase, &file()).unwrap_err();
            assert_eq!(error.code(), crate::error::Code::Usage);
            assert_eq!(error.message(), message, "{phrase:?}");
            assert_eq!(
                error.hint(),
                Some("leave only the hotkey's words in the file `--hotkey-file` names")
            );
            assert!(!error.message().contains("zzsentinel"));
        }
        let stdin = Source::File {
            flag: "--hotkey-file",
            path: PathBuf::from("-"),
        };
        let error = Hotkey::from_phrase("x", &stdin).unwrap_err();
        assert_eq!(
            error.hint(),
            Some("pipe only the hotkey's words to `--hotkey-file -`")
        );
        for source in [Source::Default, Source::Argument] {
            let error = Hotkey::from_phrase("x", &source).unwrap_err();
            assert_eq!(
                error.hint(),
                Some("pass the file that holds the hotkey's words"),
                "{source:?}"
            );
        }
    }

    #[test]
    fn debug_shows_the_address_only() {
        let debug = format!("{:?}", key(TEST_PHRASE));
        assert_eq!(
            debug,
            "Hotkey { ss58_address: \"5GmS1wtCfR4tK5SSgnZbVT4kYw5W8NmxmijcsxCQE6oLW6A8\", .. }"
        );
    }

    /// Twelve listed words in a row, in any letter case and among other text, are a seed
    /// phrase on the command line; eleven are not.
    #[test]
    fn twelve_listed_words_in_a_row_are_a_phrase() {
        let words: Vec<&str> = DEV_PHRASE.split_whitespace().collect();
        assert!(!holds_phrase(&words[..11].join(" ")));
        assert!(!holds_phrase(&format!(
            "{} x {}",
            words[..6].join(" "),
            words[6..].join(" ")
        )));
        assert!(holds_phrase(&format!("x {DEV_PHRASE} y")));
        assert!(holds_phrase(&DEV_PHRASE.to_uppercase()));
        assert!(!holds_phrase(""));
    }

    /// The refusal of a phrase on the command line repeats nothing of it and names the one
    /// input that takes such words.
    #[test]
    fn a_phrase_in_argv_is_refused_with_the_hotkey_file() {
        let error = phrase_in_argv();
        assert_eq!(error.code(), crate::error::Code::Usage);
        assert_eq!(
            error.message(),
            "the command line holds what looks like a seed phrase, so it is not repeated here; \
             treat the words as exposed to anyone who can read this machine's process list or \
             shell history"
        );
        assert_eq!(
            error.hint(),
            Some(
                "keep seed phrases off the command line: sign in with a console token at `hipc \
                 auth login`, and pass a Bittensor hotkey's words only in the file \
                 `--hotkey-file` names"
            )
        );
    }

    /// A key is found by its checksum, in any case, NFKD form, and separator, inside prose;
    /// twelve listed words whose checksum fails are not a key.
    #[test]
    fn a_checked_phrase_is_found_and_prose_is_not() {
        let phrase = TEST_PHRASE;
        let fullwidth: String = phrase
            .chars()
            // Fullwidth Latin letters, which NFKD folds back to ASCII.
            .map(|c| match c {
                'a'..='z' => char::from_u32(u32::from(c) - u32::from('a') + 0xff41).unwrap(),
                other => other,
            })
            .collect();
        for text in [
            phrase.to_owned(),
            format!("my key: {}. thanks", phrase.to_uppercase()),
            phrase.replace(' ', ", "),
            fullwidth,
        ] {
            assert!(holds_checked_phrase(&text), "{text}");
        }
        for text in [
            "please help can you check this token error when you have that file",
            "",
            "abandon",
        ] {
            assert!(!holds_checked_phrase(text), "{text}");
        }
    }

    /// PBKDF2-HMAC-SHA-512 of "password" and "salt", one iteration, 64 bytes, as published
    /// for RFC 6070's inputs with SHA-512.
    #[test]
    fn pbkdf2_hmac_sha512_matches_the_published_vector() {
        let mut out = [0_u8; 64];
        pbkdf2::pbkdf2_hmac::<Sha512>(b"password", b"salt", 1, &mut out);
        assert_eq!(
            hex(&out),
            concat!(
                "867f70cf1ade02cff3752599a3a53dc4af34c7a669815ae5d513554e1c8cf252",
                "c02d470a285a0501bad999bfe943c08f050235d7d68b1da55e63f73b60a57fce",
            )
        );
    }

    /// RFC 7693 Appendix A, BLAKE2b-512 of "abc", and an SS58 address whose checksum was
    /// computed independently: prefix 42 and the key bytes 0 to 31.
    #[test]
    fn blake2b512_and_the_ss58_checksum_match_known_vectors() {
        assert_eq!(
            hex(&Blake2b512::digest(b"abc")),
            concat!(
                "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d1",
                "7d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923",
            )
        );
        let public: [u8; 32] = std::array::from_fn(|i| u8::try_from(i).unwrap());
        assert_eq!(
            ss58(&public),
            "5C4iA2und8WV6mbvTBYupm2eZwtxk3wCYUM2SFHXSyQuapGp"
        );
    }

    #[test]
    fn only_a_challenge_passes_the_checks() {
        let key = key(TEST_PHRASE);
        let nonce = "8f2c41d09a7b";
        let ok = format!("Sign in to Hippius with challenge {nonce}");
        assert!(check_challenge(&ok, nonce).is_ok());
        let long = format!("{nonce} {}", "a".repeat(1024));
        let siwe = format!(
            "example.com wants you to sign in with your Ethereum account:\n{}\n\nNonce: {nonce}",
            key.ss58_address()
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
            (
                "example.com wants you to sign in\u{2028}Nonce: 8f2c41d09a7b",
                nonce,
                "message holds a line or paragraph separator",
            ),
            (
                "example.com wants you to sign in\u{2029}Nonce: 8f2c41d09a7b",
                nonce,
                "message holds a line or paragraph separator",
            ),
        ];
        for (message, challenge, why) in cases {
            assert_eq!(check_challenge(message, challenge), Err(why), "{message:?}");
            let error = key.sign_gamma_link(message, challenge).unwrap_err();
            assert_eq!(error.code(), crate::error::Code::Internal);
            assert!(error.message().contains(why), "{}", error.message());
        }
        // Exactly 33 and exactly 1024 bytes pass.
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
}
