//! The grammar of `hipc compute databases`.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use hippius::phrase;

use crate::cli::{DATABASE_ID, examples, number};

#[derive(Debug, Subcommand)]
pub(crate) enum Databases {
    /// Every managed database in the account; never shows a credential
    #[command(after_help = examples(&[
        "hipc compute databases list",
    ]))]
    List,
    /// One database: its status, connection, backups, and edge exposure; never shows a credential
    #[command(after_help = examples(&[
        "hipc compute databases get 7",
    ]))]
    Get {
        /// The database id from hipc compute databases list
        #[arg(value_parser = number(DATABASE_ID))]
        database_id: u64,
    },
    /// Create a database; spends money: needs --yes, and --reveal or --credentials-dir, because
    /// the answer holds credentials shown only once
    ///
    /// The database is billed from the balance by usage until it is deleted: the VMs of its tier
    /// and size, at hipc compute prices, plus the managed database fee that hipc compute
    /// public-prices lists; a dry run shows no price. The answer's
    /// credentials hold the password, the backup passphrase, and the TLS client certificate, its
    /// key, and the CA. The server keeps no copy, and no command can show them again: they are
    /// printed on stdout only, in this document, or with --credentials-dir they go only to new
    /// private files, and stdout names the files. An agent should pass --credentials-dir, so
    /// no credential reaches its transcript; without it, redirect stdout to a file only the
    /// user can read. The command does not wait: it returns with the create answer, and hipc
    /// compute databases get follows the new id to running. A launch the platform refuses
    /// leaves no database: hipc compute databases refusals says why.
    #[command(after_help = examples(&[
        "hipc compute databases create --tier starter --size s --engine-version 17 --reveal --yes",
    ]))]
    Create(DatabaseCreate),
    /// Decommission a database and every VM behind it; needs --yes
    ///
    /// A deleted database cannot be restored, since a restore starts from a database that
    /// exists: take a pg_dump first. The database's time stops being billed. The server keeps
    /// the backup bucket
    /// hippius-db-ID-backups and the S3 sub token db-ID-backup-writer it made for the database:
    /// empty the bucket with an S3 client and delete it with hipc s3 buckets delete, and revoke
    /// the token with hipc s3 tokens revoke --kind sub. In text mode the command names both
    /// when it is done.
    #[command(after_help = examples(&[
        "hipc compute databases delete 7 --yes",
    ]))]
    Delete {
        /// The database id from hipc compute databases list
        #[arg(value_parser = number(DATABASE_ID))]
        database_id: u64,
    },
    /// A drained restart of PostgreSQL, not a power cycle; needs --yes
    ///
    /// The restart is queued until the guest's next heartbeat, so the answer does not prove that
    /// PostgreSQL restarted: restart.pending in hipc compute databases get clears when it has.
    #[command(after_help = examples(&[
        "hipc compute databases restart 7 --yes",
    ]))]
    Restart {
        /// The database id from hipc compute databases list
        #[arg(value_parser = number(DATABASE_ID))]
        database_id: u64,
    },
    /// Create a new database from this one's backups, at a point in time; spends money: needs
    /// --yes, and --reveal or --credentials-dir
    ///
    /// Never in place: the source keeps serving and is not written to. The new database replays the
    /// backups up to --target-time, or to the end of the archive, and has its own credentials,
    /// which the answer may hold once, or which go only to new files with --credentials-dir.
    /// The server refuses to restore an ha database. The command does not wait: hipc compute
    /// databases get the new id.
    #[command(after_help = examples(&[
        "hipc compute databases restore 7 --reveal --yes",
        "hipc compute databases restore 7 --target-time 2026-10-27T09:30:00Z --reveal --yes",
    ]))]
    Restore(DatabaseRestore),
    /// Publish the primary on the public edge, or take it down
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute databases exposure create 7 --yes",
        "hipc compute databases exposure delete 7 --yes",
    ]))]
    Exposure(DatabaseExposure),
    /// The guest's journal, newest first
    #[command(after_help = examples(&[
        "hipc compute databases events 7",
    ]))]
    Events {
        /// The database id from hipc compute databases list
        #[arg(value_parser = number(DATABASE_ID))]
        database_id: u64,
    },
    /// The samples behind the console's charts, oldest first, the latest one, and when the guest
    /// agent last reported
    ///
    /// A field the guest could not read is null.
    #[command(after_help = examples(&[
        "hipc compute databases metrics 7",
    ]))]
    Metrics {
        /// The database id from hipc compute databases list
        #[arg(value_parser = number(DATABASE_ID))]
        database_id: u64,
    },
    /// The launches the platform refused in the last 7 days
    ///
    /// A refused create leaves no database, only an entry here.
    #[command(after_help = examples(&[
        "hipc compute databases refusals",
    ]))]
    Refusals,
}

#[derive(Debug, Args)]
pub(crate) struct DatabaseCreate {
    /// The tier: starter and production are one instance; ha is three, with failover
    #[arg(long, value_enum)]
    pub(crate) tier: DatabaseTier,
    /// The instance size
    #[arg(long, value_enum)]
    pub(crate) size: DatabaseSize,
    /// The PostgreSQL major version
    #[arg(long, value_enum)]
    pub(crate) engine_version: EngineVersion,
    /// A label
    ///
    /// The server defaults it to db-<id>.
    #[arg(long)]
    pub(crate) name: Option<String>,
    /// Print the one-time credentials on stdout; without it or --credentials-dir nothing is sent
    #[arg(long)]
    pub(crate) reveal: bool,
    /// Write the one-time credentials to new files of mode 0600 in this directory instead of
    /// printing them; in place of --reveal
    ///
    /// The files are client.crt, client.key, and ca.crt, the TLS client certificate, its key,
    /// and the CA; credentials.json, the whole credentials object as the server answered it,
    /// the password and the backup passphrase included; and .env, the libpq settings PGUSER,
    /// PGPASSWORD, PGDATABASE, PGSSLMODE, PGSSLROOTCERT, PGSSLCERT, and PGSSLKEY, with PGHOST
    /// and PGPORT when the answer has them, each value single-quoted and each path absolute.
    /// The directory must exist, must not itself be a symbolic link or writable by others, must
    /// not hold any of the five names yet, and its path must have no single quote, backslash,
    /// or control character; it is checked before anything is sent. The document keeps only the
    /// username, the database name, and shown_once of the credentials, and names the files:
    /// tls.client_cert_file, tls.client_key_file, tls.ca_cert_file, json_file, and env_file. Any
    /// other field that repeats a secret, such as a connection string, shows <redacted> in its
    /// place. Connect with psql after set -a; . <dir>/.env; set +a, adding
    /// PGHOST and PGPORT from hipc compute databases get once it runs. In a URL, percent-encode
    /// the password. If a file cannot be written after the create, the whole answer is printed
    /// instead, so no credential is lost.
    #[arg(long, value_name = "DIR", value_parser = credentials_dir)]
    pub(crate) credentials_dir: Option<PathBuf>,
}

/// The TLS files `databases create` and `restore` `--credentials-dir` write, each with the field
/// of the answer's `credentials.tls` it holds.
pub(crate) const TLS_FILES: [(&str, &str); 3] = [
    ("client.crt", "client_cert"),
    ("client.key", "client_key"),
    ("ca.crt", "ca_cert"),
];

/// The file `--credentials-dir` writes the whole `credentials` object to.
pub(crate) const CREDENTIALS_JSON: &str = "credentials.json";

/// The file `--credentials-dir` writes the libpq settings to.
pub(crate) const ENV_FILE: &str = ".env";

/// The directory the database TLS files go to: one that exists, whose last name is not a
/// symlink, that no other user can write (`commands::files::private_dir`), and that holds
/// none of [`TLS_FILES`], [`CREDENTIALS_JSON`], and [`ENV_FILE`], not even as a dangling
/// symlink. Checked at
/// parse time, so a bad one is a usage error before anything is sent. Only the last name is
/// read without following it; the directories above it are the caller's path
/// and may be links (`/tmp` is one on macOS). When the files are written, the directory rule
/// is checked again on the open directory, and the exclusive create refuses an existing file
/// or a symlink at each name.
///
/// The path is returned without a trailing `/` or a `.` part: `link/` and `link/.` name the
/// target of `link`, and the check reads the name itself, never through it.
fn credentials_dir(text: &str) -> Result<PathBuf, String> {
    let dir: PathBuf = Path::new(text).components().collect();
    let metadata = std::fs::symlink_metadata(&dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound if text.is_empty() => "name a directory".to_owned(),
        std::io::ErrorKind::NotFound => {
            "that directory does not exist: create it, or name one that does".to_owned()
        }
        _ => format!("cannot read that directory: {}", phrase::io_reason(&e)),
    })?;
    if metadata.is_symlink() {
        return Err(
            "that path is a symbolic link, and the files go only into a real directory: name \
             the directory it points to"
                .to_owned(),
        );
    }
    if !metadata.is_dir() {
        return Err("that path is not a directory: name a directory".to_owned());
    }
    crate::commands::files::private_dir_refusal(&dir, &metadata)?;
    // `.env` names the TLS files by this directory's absolute path, single-quoted: a path it
    // cannot hold so would be found only after the create, which then writes nothing.
    let absolute = std::path::absolute(&dir).ok();
    match absolute.as_deref().unwrap_or(&dir).to_str() {
        Some(path) if !path.contains(['\'', '\\']) && !path.chars().any(char::is_control) => {}
        _ => {
            return Err(
                "the `.env` written there names the files by this directory's path, which must \
                 be UTF-8 with no single quote, backslash, or control character: name another \
                 directory"
                    .to_owned(),
            );
        }
    }
    let names = TLS_FILES.map(|(file, _)| file);
    for file in names.iter().chain(&[CREDENTIALS_JSON, ENV_FILE]) {
        if std::fs::symlink_metadata(dir.join(file)).is_ok() {
            return Err(format!(
                "`{file}` is already in that directory, and hipc never overwrites it: name \
                 another directory, or move it away"
            ));
        }
    }
    Ok(dir)
}

/// The `tier` values `POST /api/databases/` accepts (swagger enum).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum DatabaseTier {
    Starter,
    Production,
    Ha,
}

impl DatabaseTier {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Starter => "starter",
            Self::Production => "production",
            Self::Ha => "ha",
        }
    }
}

/// The `size` values `POST /api/databases/` accepts (swagger enum).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum DatabaseSize {
    S,
    M,
    L,
    Xl,
}

impl DatabaseSize {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::S => "s",
            Self::M => "m",
            Self::L => "l",
            Self::Xl => "xl",
        }
    }
}

/// The PostgreSQL majors `POST /api/databases/` accepts (swagger: "16, 17 or 18").
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum EngineVersion {
    #[value(name = "16")]
    V16,
    #[value(name = "17")]
    V17,
    #[value(name = "18")]
    V18,
}

impl EngineVersion {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::V16 => "16",
            Self::V17 => "17",
            Self::V18 => "18",
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct DatabaseRestore {
    /// The database id from hipc compute databases list
    #[arg(value_parser = number(DATABASE_ID))]
    pub(crate) database_id: u64,
    /// The point in time to restore to, in RFC 3339 such as 2026-10-27T09:30:00Z
    ///
    /// Without it the backups are replayed to the end of the archive.
    #[arg(long, value_name = "TIME", value_parser = target_time)]
    pub(crate) target_time: Option<String>,
    /// Print the new database's one-time credentials on stdout; without it or --credentials-dir
    /// nothing is sent
    #[arg(long)]
    pub(crate) reveal: bool,
    /// Write the new database's one-time credentials to new files of mode 0600 in this
    /// directory instead of printing them; in place of --reveal
    ///
    /// The same files and rules as databases create --credentials-dir: client.crt, client.key,
    /// ca.crt, credentials.json, and .env, in a directory that exists, is not itself a symbolic
    /// link or writable by others, holds none of the five names yet, and has no single quote,
    /// backslash, or control character in its path; it is checked before
    /// anything is sent. The document keeps only the username, the database name, and
    /// shown_once of the credentials, and names the files. If the answer holds no TLS values,
    /// or a file cannot be written, the whole answer is printed instead, so no credential is
    /// lost.
    #[arg(long, value_name = "DIR", value_parser = credentials_dir)]
    pub(crate) credentials_dir: Option<PathBuf>,
}

/// A `--target-time`, checked here because the restore is a confirmed spend: a typo is
/// refused before the question, not by the server after it. Only the RFC 3339 shape is
/// checked (`YYYY-MM-DDTHH:MM:SS`, an optional fraction, then `Z` or `+HH:MM`); whether
/// the time is in range and inside the backups is the server's to judge. RFC 3339 also allows
/// a lowercase `t` and `z`, which are sent uppercase, as the API writes them.
fn target_time(value: &str) -> Result<String, String> {
    let value: String = value
        .chars()
        .map(|c| match c {
            't' => 'T',
            'z' => 'Z',
            c => c,
        })
        .collect();
    let shape: String = value
        .chars()
        .map(|c| if c.is_ascii_digit() { '0' } else { c })
        .collect();
    let zone =
        shape
            .strip_prefix("0000-00-00T00:00:00")
            .and_then(|rest| match rest.strip_prefix('.') {
                // A fraction has at least one digit.
                Some(fraction) if fraction.starts_with('0') => {
                    Some(fraction.trim_start_matches('0'))
                }
                Some(_) => None,
                None => Some(rest),
            });
    if matches!(zone, Some("Z" | "+00:00" | "-00:00")) {
        Ok(value)
    } else {
        Err("a target time is RFC 3339, such as `2026-10-27T09:30:00Z`".to_owned())
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum DatabaseExposure {
    /// Publish the primary on the public edge; needs --yes
    ///
    /// The edge passes TLS through by SNI and holds no key, so the session is end to end with the
    /// database, and the password is still required. The certificates are the ones the create
    /// issued; nothing new is minted.
    #[command(after_help = examples(&[
        "hipc compute databases exposure create 7 --yes",
    ]))]
    Create {
        /// The database id from hipc compute databases list
        #[arg(value_parser = number(DATABASE_ID))]
        database_id: u64,
    },
    /// Take the primary off the public edge; needs --yes
    #[command(after_help = examples(&[
        "hipc compute databases exposure delete 7 --yes",
    ]))]
    Delete {
        /// The database id from hipc compute databases list
        #[arg(value_parser = number(DATABASE_ID))]
        database_id: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_time_has_the_rfc_3339_shape() {
        for good in [
            "2026-10-27T09:30:00Z",
            "2026-10-27T09:30:00.5Z",
            "2026-10-27T09:30:00.123456+02:00",
            "2026-10-27T09:30:00-05:30",
        ] {
            assert_eq!(target_time(good).as_deref(), Ok(good));
        }
        assert_eq!(
            target_time("2026-10-27t09:30:00.5z").as_deref(),
            Ok("2026-10-27T09:30:00.5Z")
        );
        for bad in [
            "",
            "2026-10-27",
            "yesterday",
            "2026-10-27 09:30:00Z",
            "2026-10-27T09:30Z",
            "2026-10-27T09:30:00",
            "2026-10-27T09:30:00.Z",
            "2026-10-27T09:30:00+0200",
            "2026-10-27T09:30:00Zx",
            "02026-10-27T09:30:00Z",
            "dddd-dd-ddTdd:dd:ddZ",
            "2026-dd-27T09:30:00Z",
        ] {
            assert!(target_time(bad).is_err(), "{bad}");
        }
    }
}
