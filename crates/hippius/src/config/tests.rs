use std::fs;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};

use super::*;
use crate::private_file::MAX_FILE_BYTES;

/// A fresh directory with mode 0700, whatever the umask. Ubuntu's default umask, 0002,
/// would leave it writable by the group, which hipc refuses on purpose.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hipc-config-{name}-{}", std::process::id()));
    drop(fs::remove_dir_all(&dir));
    fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

/// A regular file root owns, in a directory that is not a symlink, so that opening it
/// is refused for its owner and not for its shape. `/usr/bin/env` is a symlink on some
/// Linux hosts; `/etc` is one on macOS, where `/private/etc` is the directory.
fn a_root_owned_regular_file() -> PathBuf {
    const CANDIDATES: [&str; 5] = [
        "/usr/bin/env",
        "/etc/passwd",
        "/etc/hosts",
        "/private/etc/passwd",
        "/private/etc/hosts",
    ];
    CANDIDATES
        .into_iter()
        .map(PathBuf::from)
        .find(|path| {
            fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.uid() == 0)
                && fs::symlink_metadata(path.parent().unwrap()).is_ok_and(|m| m.is_dir())
        })
        .expect("no root-owned regular file in a real directory among CANDIDATES")
}

/// Saves `config` to its path without the lock, as one step of [`ConfigFile::edit`].
fn save(config: &ConfigFile) -> Result<()> {
    config.save(&PrivateFile::ensure(&config.path, CONFIG_FILE)?)
}

#[test]
fn a_stored_token_for_another_origin_is_withheld_not_sent() {
    let mut file = ConfigFile {
        path: PathBuf::from("unused"),
        document: Document::default(),
    };
    file.set_token(
        "default",
        &Token::new("stored-token", &Source::Argument).unwrap(),
        DEFAULT_API_URL,
    )
    .unwrap();
    let elsewhere = Overrides {
        api_url: Some(Given {
            value: "https://staging.example".into(),
            source: Source::Flag("--api-url"),
        }),
        ..Overrides::default()
    };
    let settings = file.settings(&elsewhere).unwrap();
    assert!(
        settings.token().is_none(),
        "the stored token must not go elsewhere"
    );
    assert!(matches!(
        &settings.credential,
        Credential::Withheld { stored_for } if stored_for == DEFAULT_API_URL
    ));
}

#[test]
fn a_token_must_be_one_word_of_visible_ascii() {
    assert_eq!(
        Token::new(" tok_abc \n", &Source::Argument)
            .unwrap()
            .expose(),
        "tok_abc"
    );
    for bad in ["tok_a\ntok_b", "tok a", "tok\u{7f}", "tök"] {
        assert_eq!(
            Token::new(bad, &Source::Argument).unwrap_err().code(),
            crate::Code::Usage,
            "{bad:?}"
        );
    }
}

#[test]
fn path_is_hippius_config_else_dot_hippius_in_home() {
    let explicit = config_path(Some("/x/c.toml".into()), Some("/home/u".into())).unwrap();
    assert_eq!(explicit, PathBuf::from("/x/c.toml"));
    let home = config_path(None, Some("/home/u".into())).unwrap();
    assert_eq!(home, PathBuf::from("/home/u/.hippius/config.toml"));
    let empty = config_path(Some(PathBuf::new()), Some("/home/u".into())).unwrap();
    assert_eq!(empty, home);
    assert!(config_path(None, None).is_err());
    assert!(config_path(None, Some(PathBuf::new())).is_err());
}

#[test]
fn flag_then_env_then_profile() {
    let mut file = ConfigFile {
        path: PathBuf::from("unused"),
        document: Document::default(),
    };
    file.set_token(
        "default",
        &Token::new("stored-token", &Source::Argument).unwrap(),
        DEFAULT_API_URL,
    )
    .unwrap();
    let settings = file.settings(&Overrides::default()).unwrap();
    assert_eq!(settings.token().unwrap().expose(), "stored-token");
    assert_eq!(settings.api_url, DEFAULT_API_URL);
    assert!(matches!(settings.account, Selection::Own));

    let overrides = Overrides {
        token: Some(Token::new(" env-token\n", &Source::Argument).unwrap()),
        api_url: Some(Given {
            value: "http://127.0.0.1:9/".into(),
            source: Source::Flag("--api-url"),
        }),
        ..Overrides::default()
    };
    // Bound to the default origin, the variable is not sent to another, and the stored
    // token does not stand in for it.
    let settings = file.settings(&overrides).unwrap();
    assert!(settings.token().is_none());
    assert!(matches!(
        &settings.credential,
        Credential::VariableWithheld { bound_to } if bound_to == DEFAULT_API_URL
    ));
    assert_eq!(settings.api_url, "http://127.0.0.1:9");
    // `HIPPIUS_TOKEN_ORIGIN` names the target, in any spelling of it.
    for spelling in [
        "http://127.0.0.1:9",
        "HTTP://127.0.0.1:9/",
        "http://127.0.0.1:09",
    ] {
        let bound = Overrides {
            token: overrides.token.clone(),
            api_url: overrides.api_url.clone(),
            token_origin: Some(Given {
                value: spelling.into(),
                source: Source::Variable("HIPPIUS_TOKEN_ORIGIN"),
            }),
            ..Overrides::default()
        };
        let settings = file.settings(&bound).unwrap();
        assert_eq!(
            settings.token().unwrap().expose(),
            "env-token",
            "{spelling}"
        );
    }
}

/// `HIPPIUS_TOKEN` goes only to the origin `HIPPIUS_TOKEN_ORIGIN` names, else the default
/// one: another target, a port, a scheme, or a host off is withheld, and a value that is
/// no origin is refused, naming the variable.
#[test]
fn the_token_variable_is_sent_only_to_the_origin_it_is_bound_to() {
    let file = ConfigFile {
        path: PathBuf::from("unused"),
        document: Document::default(),
    };
    let run = |target: Option<&str>, bound: Option<&str>| {
        file.settings(&Overrides {
            token: Some(Token::new("env-token", &Source::Argument).unwrap()),
            api_url: target.map(|value| given(value, "--api-url")),
            token_origin: bound.map(|value| Given {
                value: value.to_owned(),
                source: Source::Variable("HIPPIUS_TOKEN_ORIGIN"),
            }),
            ..Overrides::default()
        })
    };
    let sent = |target, bound| run(target, bound).unwrap().token().is_some();
    assert!(sent(None, None));
    assert!(sent(Some("https://API.hippius.com:443/"), None));
    assert!(sent(None, Some("https://api.hippius.com")));
    assert!(!sent(Some("https://collector.example"), None));
    assert!(!sent(
        Some("https://collector.example"),
        Some("https://other.example")
    ));
    assert!(!sent(
        Some("http://127.0.0.1:8"),
        Some("http://127.0.0.1:9")
    ));
    assert!(!sent(
        Some("http://127.0.0.1:9"),
        Some("https://127.0.0.1:9")
    ));
    // Bound elsewhere, the variable is withheld from the default origin too.
    assert!(!sent(None, Some("http://127.0.0.1:9")));
    assert!(sent(
        Some("https://collector.example/"),
        Some("HTTPS://Collector.example")
    ));
    let error = run(Some("https://collector.example"), Some("collector.example")).unwrap_err();
    assert_eq!(error.code(), crate::Code::Usage);
    let message = error.message();
    assert!(message.contains("`HIPPIUS_TOKEN_ORIGIN`"), "{message}");
}

const PROJECT: &str = "proj_0a1b2c3d4e5f60718293";
const BILLING: &str = "bill_9f8e7d6c5b4a39281706";

fn given(value: &str, flag: &'static str) -> Given {
    Given {
        value: value.to_owned(),
        source: Source::Flag(flag),
    }
}

/// The project and the billing account are picked as the account is: an override, then
/// the profile, then the Default one; `default` from an override wins over a stored id.
#[test]
fn the_project_and_billing_account_come_from_an_override_then_the_profile() {
    let mut file = ConfigFile {
        path: PathBuf::from("unused"),
        document: Document::default(),
    };
    let settings = file.settings(&Overrides::default()).unwrap();
    assert!(settings.project.is_none() && settings.billing.is_none());

    let project = ProjectId::parse(PROJECT, &Source::Argument).unwrap();
    let billing = BillingId::parse(BILLING, &Source::Argument).unwrap();
    file.set_project("default", project.as_ref());
    file.set_billing("default", billing.as_ref());
    let settings = file.settings(&Overrides::default()).unwrap();
    assert_eq!(settings.project.unwrap().as_str(), PROJECT);
    assert_eq!(settings.billing.unwrap().as_str(), BILLING);

    let overrides = Overrides {
        project: Some(given("default", "--project")),
        billing: Some(given("default", "--billing")),
        ..Overrides::default()
    };
    let settings = file.settings(&overrides).unwrap();
    assert!(settings.project.is_none() && settings.billing.is_none());

    let other = "proj_ffffffffffffffffffff";
    let overrides = Overrides {
        project: Some(given(other, "--project")),
        ..Overrides::default()
    };
    assert_eq!(
        file.settings(&overrides).unwrap().project.unwrap().as_str(),
        other
    );

    file.set_project("default", None);
    file.set_billing("default", None);
    let settings = file.settings(&Overrides::default()).unwrap();
    assert!(settings.project.is_none() && settings.billing.is_none());
}

/// A malformed id is refused whether an override or the profile holds it, and the
/// refusal names the one that did.
#[test]
fn a_malformed_project_or_billing_account_names_where_it_came_from() {
    let mut file = ConfigFile {
        path: PathBuf::from("/h/config.toml"),
        document: Document::default(),
    };
    let overrides = Overrides {
        billing: Some(given("bill_x", "--billing")),
        ..Overrides::default()
    };
    let error = file.settings(&overrides).unwrap_err();
    assert!(error.message().contains("from `--billing`"), "{error}");
    file.profile_mut("default").project = Some("proj_x".to_owned());
    let error = file.settings(&Overrides::default()).unwrap_err();
    assert!(
        error
            .message()
            .contains("from the `project` of the profile `default`"),
        "{error}"
    );
}

/// A project and a billing account belong to the account they were picked in, so a
/// new account removes them, and picking the same account again keeps them.
#[test]
fn a_new_account_clears_the_stored_project_and_billing_account() {
    let mut file = ConfigFile {
        path: PathBuf::from("unused"),
        document: Document::default(),
    };
    let member = Selection::parse("acct_0123456789abcdef0123", &Source::Argument).unwrap();
    let project = ProjectId::parse(PROJECT, &Source::Argument).unwrap();
    let billing = BillingId::parse(BILLING, &Source::Argument).unwrap();
    let pick = |file: &mut ConfigFile| {
        file.set_project("default", project.as_ref());
        file.set_billing("default", billing.as_ref());
    };
    pick(&mut file);
    file.set_account("default", &Selection::Own);
    assert!(
        file.settings(&Overrides::default())
            .unwrap()
            .project
            .is_some()
    );
    file.set_account("default", &member);
    let settings = file.settings(&Overrides::default()).unwrap();
    assert!(settings.project.is_none() && settings.billing.is_none());
    pick(&mut file);
    file.set_account("default", &member);
    let settings = file.settings(&Overrides::default()).unwrap();
    assert!(settings.project.is_some() && settings.billing.is_some());
    file.set_account("default", &Selection::Own);
    let settings = file.settings(&Overrides::default()).unwrap();
    assert!(settings.project.is_none() && settings.billing.is_none());
}

/// A project is cleared only while the profile still stores it: another project, a
/// profile without one, and a profile that does not exist are left as they are.
#[test]
fn a_project_is_cleared_only_while_it_is_still_stored() {
    let mut file = ConfigFile {
        path: PathBuf::from("unused"),
        document: Document::default(),
    };
    let stored = |file: &ConfigFile| file.document.profiles["default"].project.clone();
    assert!(!file.clear_project_if("default", PROJECT));
    file.set_project("default", None);
    assert!(!file.clear_project_if("default", PROJECT));
    let project = ProjectId::parse(PROJECT, &Source::Argument).unwrap();
    file.set_project("default", project.as_ref());
    assert!(!file.clear_project_if("default", "proj_ffffffffffffffffffff"));
    assert!(!file.clear_project_if("other", PROJECT));
    assert_eq!(stored(&file).as_deref(), Some(PROJECT));
    assert!(file.clear_project_if("default", PROJECT));
    assert_eq!(stored(&file), None);
}

/// The stored project and billing account are read only while the run acts in the
/// stored account: an override that names another account leaves them unread, and one
/// that names the stored account, `self` for none, keeps them.
#[test]
fn another_account_override_leaves_the_stored_project_unread() {
    let mut file = ConfigFile {
        path: PathBuf::from("unused"),
        document: Document::default(),
    };
    let project = ProjectId::parse(PROJECT, &Source::Argument).unwrap();
    let billing = BillingId::parse(BILLING, &Source::Argument).unwrap();
    file.set_project("default", project.as_ref());
    file.set_billing("default", billing.as_ref());
    let other = "acct_ffffffffffffffffffff";
    let run_in = |account: &str| {
        let overrides = Overrides {
            account: Some(given(account, "--account")),
            ..Overrides::default()
        };
        let settings = file.settings(&overrides).unwrap();
        (settings.project.is_some(), settings.billing.is_some())
    };
    assert_eq!(run_in("self"), (true, true));
    assert_eq!(run_in(other), (false, false));
    // A flag still selects a project in any account.
    let overrides = Overrides {
        account: Some(given(other, "--account")),
        project: Some(given(PROJECT, "--project")),
        ..Overrides::default()
    };
    assert!(file.settings(&overrides).unwrap().project.is_some());
}

/// A project or billing account is stored only for the account the profile acts in:
/// an override that names another is refused, by flag or by variable, and one that names
/// the profile's account, `self` for none, is not.
#[test]
fn a_selection_is_stored_only_for_the_profiles_account() {
    let mut file = ConfigFile {
        path: PathBuf::from("unused"),
        document: Document::default(),
    };
    let (member, other) = ("acct_0123456789abcdef0123", "acct_ffffffffffffffffffff");
    let with = |account: Option<Given>| Overrides {
        account,
        ..Overrides::default()
    };
    assert_eq!(file.selection_profile(&with(None)).unwrap(), "default");
    let own = with(Some(given("self", "--account")));
    assert_eq!(file.selection_profile(&own).unwrap(), "default");
    let error = file
        .selection_profile(&with(Some(given(member, "--account"))))
        .unwrap_err();
    assert_eq!(error.code().as_str(), "usage");
    assert_eq!(
        error.message(),
        format!(
            "`--account` selects the account `{member}`, and the profile `default` acts in \
             the token's own account: a project or billing account is stored for the \
             profile's account"
        )
    );
    assert_eq!(
        error.hint(),
        Some(format!("run `hipc account use {member}` first").as_str())
    );

    file.set_account(
        "default",
        &Selection::parse(member, &Source::Argument).unwrap(),
    );
    let same = with(Some(given(member, "--account")));
    assert_eq!(file.selection_profile(&same).unwrap(), "default");
    assert_eq!(file.selection_profile(&with(None)).unwrap(), "default");
    let variable = Given {
        value: other.to_owned(),
        source: Source::Variable("HIPPIUS_ACCOUNT"),
    };
    let error = file.selection_profile(&with(Some(variable))).unwrap_err();
    assert_eq!(
        error.message(),
        format!(
            "`HIPPIUS_ACCOUNT` selects the account `{other}`, and the profile `default` acts \
             in the account `{member}`: a project or billing account is stored for the \
             profile's account"
        )
    );
    let error = file.selection_profile(&own).unwrap_err();
    assert!(
        error
            .message()
            .starts_with("`--account` selects the account `self`"),
        "{error}"
    );
    assert_eq!(error.hint(), Some("run `hipc account use self` first"));
    // A malformed override is refused as `settings` refuses it.
    let error = file
        .selection_profile(&with(Some(given("acct_x", "--account"))))
        .unwrap_err();
    assert!(
        error.message().starts_with("`acct_x` from `--account`"),
        "{error}"
    );
}

#[test]
fn save_writes_mode_0600_in_a_0700_directory_and_round_trips() {
    let dir = temp_dir("save");
    let path = dir.join("hipc").join("config.toml");
    let mut file = ConfigFile::load(path.clone()).unwrap();
    file.set_token(
        "work",
        &Token::new("t0ken", &Source::Argument).unwrap(),
        DEFAULT_API_URL,
    )
    .unwrap();
    save(&file).unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);

    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let refused = ConfigFile::load(path.clone()).unwrap_err();
    assert_eq!(refused.code(), crate::Code::Usage);
    assert!(refused.hint().unwrap().contains("chmod 600"));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut file = ConfigFile::load(path.clone()).unwrap();
    assert!(file.has_token("work"));
    file.clear_token("work");
    save(&file).unwrap();
    assert_eq!(mode(&path), 0o600);
    assert!(!ConfigFile::load(path).unwrap().has_token("work"));
    fs::remove_dir_all(dir).unwrap();
}

/// A change keeps the owner's mode the file has now: a file the user made read-only
/// stays read-only, and no change widens a mode.
#[test]
fn a_change_keeps_the_owner_mode_the_file_has() {
    let dir = temp_dir("keep-mode");
    let path = dir.join("hipc").join("config.toml");
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    save(&ConfigFile::load(path.clone()).unwrap()).unwrap();
    for kept in [0o400, 0o700, 0o600] {
        fs::set_permissions(&path, fs::Permissions::from_mode(kept)).unwrap();
        ConfigFile::edit(path.clone(), |config| {
            config.set_account("default", &Selection::Own);
            Ok(())
        })
        .unwrap();
        assert_eq!(mode(&path), kept, "{kept:o}");
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_directory_others_can_write_is_refused_for_reads_and_writes() {
    let dir = temp_dir("shared");
    let path = dir.join("config.toml");
    let mut file = ConfigFile::load(path.clone()).unwrap();
    file.set_token(
        "default",
        &Token::new("t", &Source::Argument).unwrap(),
        DEFAULT_API_URL,
    )
    .unwrap();
    save(&file).unwrap();
    for mode in [0o770, 0o707, 0o777] {
        fs::set_permissions(&dir, fs::Permissions::from_mode(mode)).unwrap();
        let read = ConfigFile::load(path.clone()).unwrap_err();
        assert!(read.hint().unwrap().contains("chmod 700"), "{mode:o}");
        let write = save(&file).unwrap_err();
        assert!(write.hint().unwrap().contains("chmod 700"), "{mode:o}");
    }
    // Readable by others is not a reason to refuse the directory, only writable.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    ConfigFile::load(path).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn every_mode_bit_for_others_or_the_group_is_refused() {
    let dir = temp_dir("modes");
    let path = dir.join("config.toml");
    save(&ConfigFile::load(path.clone()).unwrap()).unwrap();
    for bit in [0o040, 0o020, 0o010, 0o004, 0o002, 0o001] {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600 | bit)).unwrap();
        assert!(ConfigFile::load(path.clone()).is_err(), "{:o}", 0o600 | bit);
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
    ConfigFile::load(path).unwrap();
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn save_replaces_a_stale_temporary_file_and_never_follows_a_symlink() {
    let dir = temp_dir("stale");
    let path = dir.join("config.toml");
    let temporary = dir.join(format!(".config.toml.{}.tmp", std::process::id()));
    let target = dir.join("elsewhere");
    std::os::unix::fs::symlink(&target, &temporary).unwrap();
    let mut file = ConfigFile::load(path.clone()).unwrap();
    file.set_token(
        "default",
        &Token::new("t", &Source::Argument).unwrap(),
        DEFAULT_API_URL,
    )
    .unwrap();
    save(&file).unwrap();
    assert!(!target.exists(), "the write followed the planted symlink");
    assert!(!temporary.exists());
    assert!(ConfigFile::load(path).unwrap().has_token("default"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_missing_file_is_an_empty_config_and_a_directory_is_refused() {
    let dir = temp_dir("missing");
    let file = ConfigFile::load(dir.join("nope.toml")).unwrap();
    assert!(!file.has_token("default"));
    assert_eq!(file.path(), dir.join("nope.toml"));
    // A directory that does not exist yet is the same empty config.
    let file = ConfigFile::load(dir.join("absent").join("nope.toml")).unwrap();
    assert!(!file.has_token("default"));
    fs::create_dir_all(dir.join("adir.toml")).unwrap();
    fs::set_permissions(dir.join("adir.toml"), fs::Permissions::from_mode(0o700)).unwrap();
    let error = ConfigFile::load(dir.join("adir.toml")).unwrap_err();
    assert_eq!(error.code(), crate::Code::Usage);
    assert!(error.to_string().contains("not a regular file"), "{error}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_symlinked_config_file_is_refused_and_its_target_is_not_read() {
    let dir = temp_dir("symlink");
    let target = dir.join("real.toml");
    fs::write(&target, "").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.join("config.toml");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let error = ConfigFile::load(link).unwrap_err();
    assert!(error.to_string().contains("symlink"), "{error}");
    assert_eq!(error.code(), crate::Code::Usage, "{error}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_file_or_directory_owned_by_another_user_is_refused() {
    // A file root owns, chosen for the host, and `/usr`, which root owns on every macOS
    // and Linux host. The suite does not run as root: root owns them too, and root also
    // reads a mode-000 file.
    assert!(
        !rustix::process::geteuid().is_root(),
        "run the tests as a normal user"
    );
    let error = ConfigFile::load(a_root_owned_regular_file()).unwrap_err();
    assert!(error.to_string().contains("another user"), "{error}");
    assert_eq!(error.code(), crate::Code::Usage, "{error}");
    let mut file = ConfigFile {
        path: PathBuf::from("/usr/hipc-config-test.toml"),
        document: Document::default(),
    };
    file.set_token(
        "default",
        &Token::new("t", &Source::Argument).unwrap(),
        DEFAULT_API_URL,
    )
    .unwrap();
    let error = save(&file).unwrap_err();
    assert!(error.to_string().contains("another user"), "{error}");
    assert_eq!(error.code(), crate::Code::Usage, "{error}");
    assert!(!Path::new("/usr/hipc-config-test.toml").exists());
}

#[test]
fn a_file_that_is_not_utf8_is_a_usage_error() {
    let dir = temp_dir("not-utf8");
    let path = dir.join("config.toml");
    fs::write(&path, b"[profiles.default]\ntoken = \"\xff\"\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let error = ConfigFile::load(path.clone()).unwrap_err();
    assert_eq!(error.code(), crate::Code::Usage);
    let quoted = format!("`{}`", path.display());
    assert_eq!(
        error.to_string(),
        format!("the config file {quoted} is not UTF-8 text")
    );
    assert_eq!(
        error.hint(),
        Some(format!("save {quoted} as UTF-8 text").as_str())
    );
    fs::remove_dir_all(dir).unwrap();
}

/// A writer killed between its temporary file and the rename leaves a full copy of the
/// tokens beside the config, under its own pid, which no later run would reuse. The next
/// edit removes it under the lock, and nothing else.
#[test]
fn an_edit_removes_the_temporary_files_a_crashed_writer_left() {
    let dir = temp_dir("stale-tmp");
    let stale = [".config.toml.1.tmp", ".config.toml.4294967295.tmp"];
    let kept = [
        ".config.toml.x.tmp",
        ".config.toml..tmp",
        ".other.toml.1.tmp",
        "config.toml.1.tmp",
        ".config.toml.1.tmp.bak",
    ];
    for name in stale.iter().chain(&kept) {
        fs::write(dir.join(name), "token = \"t0ken\"").unwrap();
    }
    store_token(&dir.join("config.toml")).unwrap();
    for name in stale {
        assert!(!dir.join(name).exists(), "{name} is left");
    }
    for name in kept {
        assert!(dir.join(name).exists(), "{name} was removed");
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_failed_write_leaves_no_temporary_file_and_reports_the_path() {
    let dir = temp_dir("failed");
    fs::create_dir_all(dir.join("config.toml")).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    // The rename fails: the target is a directory that is not empty.
    fs::write(dir.join("config.toml").join("keep"), "").unwrap();
    let file = ConfigFile {
        path: dir.join("config.toml"),
        document: Document::default(),
    };
    let error = save(&file).unwrap_err();
    assert_eq!(error.code(), crate::Code::Internal);
    assert!(error.to_string().contains("config.toml"), "{error}");
    let leftovers: Vec<_> = fs::read_dir(&dir).unwrap().collect();
    assert_eq!(leftovers.len(), 1, "only the directory remains");
    // The create fails: the directory is read-only, which a `chmod` fixes.
    skip_if_root!();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    let other = ConfigFile {
        path: dir.join("other.toml"),
        document: Document::default(),
    };
    let error = save(&other).unwrap_err();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(error.code(), crate::Code::Usage);
    assert_eq!(
        error.hint(),
        Some(&*format!("run `chmod 700 {}`", dir.display()))
    );
    fs::remove_dir_all(dir).unwrap();
}

/// A key or a value the config types do not take is named for what is wrong with it,
/// in `hipc`'s words, and never quoted: the key or the value can be a token.
#[test]
fn a_config_error_says_what_is_wrong_without_quoting_the_file() {
    for (text, problem) in [
        (
            "[profiles.default]\ntoken = 5\n",
            "line 2 has a value that is not a string",
        ),
        (
            "[profiles]\ndefault = \"s3cr3t\"\n",
            "line 2 has a value that is not a table",
        ),
        (
            "[profiles.default.hub_subscribe]\nat = \"s3cr3t\"\nkey = \"k\"\n",
            "line 2 has a value that is not a whole number",
        ),
        (
            "[profiles.default.hub_subscribe]\nat = 1\nkey = \"k\"\ncancels = 1\n",
            "line 4 has a value that is not a list",
        ),
        (
            "[profiles.default.hub_subscribe]\nat = 1\n",
            "line 1 lacks the key `key`",
        ),
        ("token = \"s3cr3t\" [[[", "line 1 does not parse"),
        (
            "default_profile = 3\n",
            "line 1 has a value that is not a string",
        ),
        (
            "default_profile = 1979-05-27\n",
            "line 1 has a value that is not a string",
        ),
    ] {
        let error = ConfigFile::parse(PathBuf::from("c.toml"), Some(Zeroizing::new(text.into())))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("the config file `c.toml` is not valid: {problem}"),
            "{text}"
        );
        assert!(!error.to_string().contains("s3cr3t"), "{error}");
    }
}

/// A key this `hipc` does not know, at any level, is kept as it was through a load and a
/// save, and named by where it is in a warning taken once, never by its name or value.
#[test]
fn unknown_keys_are_kept_and_warned_about_once_without_being_quoted() {
    drop(take_unknown_keys_warning());
    let text = "tok_s3cr3t = 1\n\n[profiles.default]\ntoken = \"x\"\napi_version = 2\n\n\
                [profiles.default.hub_subscribe]\nat = 1\nkey = \"k\"\nseen = \"s3cr3t\"\n\n\
                [profiles.plain]\naccount = \"a\"\n\n[profiles.\"we ird\".extra]\nnested = true\n";
    let config =
        ConfigFile::parse(PathBuf::from("c.toml"), Some(Zeroizing::new(text.into()))).unwrap();
    assert_eq!(
        take_unknown_keys_warning().as_deref(),
        Some(
            "the config file `c.toml` holds keys this hipc does not know, in the top level, \
             the profile `default`, the `hub_subscribe` of the profile `default`, and the \
             profile `we ird`; they are kept as they are, and a newer hipc may use them"
        )
    );
    assert_eq!(take_unknown_keys_warning(), None, "taken once");
    assert!(!format!("{config:?}").contains("s3cr3t"));
    let saved = toml::to_string(&config.document).unwrap();
    let again: toml::Table = toml::from_str(&saved).unwrap();
    assert_eq!(again, toml::from_str::<toml::Table>(text).unwrap());
    ConfigFile::parse(
        PathBuf::from("c.toml"),
        Some(Zeroizing::new("[profiles.default]\n".into())),
    )
    .unwrap();
    assert_eq!(take_unknown_keys_warning(), None, "a file of known keys");
}

/// A hand-edited token with spaces around it is sent trimmed, so a login with the same
/// token does not replace another one, and a blank one is no token.
#[test]
fn a_stored_token_is_compared_trimmed() {
    let text = "[profiles.padded]\ntoken = \"  tok_same_0123456789 \"\n\
                [profiles.blank]\ntoken = \"   \"\n";
    let config =
        ConfigFile::parse(PathBuf::from("c.toml"), Some(Zeroizing::new(text.into()))).unwrap();
    let same = Token::new("tok_same_0123456789", &Source::Argument).unwrap();
    let other = Token::new("tok_other_0123456789", &Source::Argument).unwrap();
    assert!(!config.stores_other_token("padded", &same));
    assert!(config.stores_other_token("padded", &other));
    assert!(!config.stores_other_token("blank", &other));
}

#[test]
fn debug_never_prints_a_token() {
    let token = Token::new("super-secret-value", &Source::Argument).unwrap();
    assert!(!format!("{token:?}").contains("secret"));
    let mut file = ConfigFile {
        path: PathBuf::from("unused"),
        document: Document::default(),
    };
    file.set_token("default", &token, DEFAULT_API_URL).unwrap();
    assert!(!format!("{file:?}").contains("secret"));
    let overrides = Overrides {
        token: Some(token.clone()),
        ..Overrides::default()
    };
    assert!(!format!("{overrides:?}").contains("secret"));
    assert_eq!(token.preview().as_deref(), Some("alue"));
    assert_eq!(
        Token::new("12345678901", &Source::Argument)
            .unwrap()
            .preview(),
        None
    );
    assert_eq!(
        Token::new("123456789012", &Source::Argument)
            .unwrap()
            .preview()
            .as_deref(),
        Some("9012")
    );
}

#[test]
fn a_bad_file_names_the_path_and_not_the_contents() {
    let dir = temp_dir("bad");
    let path = dir.join("config.toml");
    fs::write(&path, "token = \"leak-me\"\nnot toml [").unwrap();
    // Mode 0600, or the mode refusal fires first and the parse is never reached.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let error = ConfigFile::load(path).unwrap_err();
    assert!(error.to_string().contains("config.toml"));
    assert!(error.to_string().contains("not valid"), "{error}");
    assert!(!error.to_string().contains("leak-me"));
    fs::remove_dir_all(dir).unwrap();
}

/// Stores a token through [`ConfigFile::edit`], the path every command that writes
/// takes.
fn store_token(path: &Path) -> Result<()> {
    ConfigFile::edit(path.to_owned(), |config| {
        let token = Token::new("t0ken", &Source::Argument)?;
        config
            .set_token("default", &token, DEFAULT_API_URL)
            .map(drop)
    })
}

/// An edit of a file with another hard link is noted once, for the binary's warning: the
/// rename leaves the old contents, the token included, in the other link. A file with one
/// link, and a new file, are not noted.
#[test]
fn an_edit_of_a_hard_linked_config_is_noted_once() {
    let dir = temp_dir("hard-link");
    let path = dir.join("config.toml");
    drop(take_split_link());
    store_token(&path).unwrap();
    assert_eq!(take_split_link(), None, "a new file");
    store_token(&path).unwrap();
    assert_eq!(take_split_link(), None, "one link");
    let other = dir.join("dotfiles-copy.toml");
    fs::hard_link(&path, &other).unwrap();
    store_token(&path).unwrap();
    assert_eq!(take_split_link(), Some(path.clone()));
    assert_eq!(take_split_link(), None, "taken once");
    assert!(fs::read_to_string(&other).unwrap().contains("t0ken"));
    // The rename split the link, so the next edit has one again.
    store_token(&path).unwrap();
    assert_eq!(take_split_link(), None);
}

#[test]
fn a_symlinked_config_directory_is_refused_for_reads_and_writes() {
    let dir = temp_dir("linked-dir");
    let real = dir.join("real");
    fs::create_dir_all(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    store_token(&real.join("config.toml")).unwrap();
    let before = fs::read(real.join("config.toml")).unwrap();
    std::os::unix::fs::symlink(&real, dir.join(".hippius")).unwrap();
    let path = dir.join(".hippius").join("config.toml");
    let hint = "set `HIPPIUS_CONFIG` to a file in the directory the symlink points to";

    let read = ConfigFile::load(path.clone()).unwrap_err();
    assert_eq!(read.code(), crate::Code::Usage, "{read}");
    assert!(read.to_string().contains("is a symlink"), "{read}");
    assert_eq!(read.hint(), Some(hint));

    let write = store_token(&path).unwrap_err();
    assert_eq!(write.code(), crate::Code::Usage, "{write}");
    assert_eq!(write.hint(), Some(hint));
    assert_eq!(fs::read(real.join("config.toml")).unwrap(), before);
    let mut names: Vec<_> = fs::read_dir(&real)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [".config.toml.lock", "config.toml"],
        "nothing was written"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_config_directory_that_is_a_file_or_unreachable_is_refused() {
    let dir = temp_dir("not-a-dir");
    fs::write(dir.join("file"), "").unwrap();
    let error = store_token(&dir.join("file").join("config.toml")).unwrap_err();
    assert_eq!(error.code(), crate::Code::Usage, "{error}");
    assert!(error.to_string().contains("is not a directory"), "{error}");
    assert_eq!(
        error.hint(),
        Some("set `HIPPIUS_CONFIG` to a file in a directory")
    );

    // The owner may not search the parent, so the directory in it cannot be opened.
    fs::create_dir_all(dir.join("closed").join("hipc")).unwrap();
    fs::set_permissions(dir.join("closed"), fs::Permissions::from_mode(0o600)).unwrap();
    let error = store_token(&dir.join("closed").join("hipc").join("config.toml"));
    fs::set_permissions(dir.join("closed"), fs::Permissions::from_mode(0o700)).unwrap();
    let error = error.unwrap_err();
    // A mode the person changes, not a bug to report.
    assert_eq!(error.code(), crate::Code::Usage, "{error}");
    assert_eq!(
        error.to_string(),
        format!(
            "cannot open {}: permission is denied",
            quote_path(&dir.join("closed").join("hipc"))
        )
    );
    assert_eq!(
        error.hint(),
        Some(&*format!(
            "check that the current user can enter {} and each directory above it",
            quote_path(&dir.join("closed").join("hipc"))
        ))
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_path_that_names_no_file_is_refused_before_anything_is_written() {
    let dir = temp_dir("no-name");
    fs::create_dir_all(dir.join("sub")).unwrap();
    let error = store_token(&dir.join("sub").join("..")).unwrap_err();
    assert_eq!(error.code(), crate::Code::Usage, "{error}");
    assert!(error.to_string().contains("names no file"), "{error}");
    assert_eq!(error.hint(), Some("set `HIPPIUS_CONFIG` to a file path"));
    assert_eq!(fs::read_dir(dir.join("sub")).unwrap().count(), 0);
    // A missing directory is not created for such a path either.
    let error = store_token(&dir.join("new").join("..")).unwrap_err();
    assert!(error.to_string().contains("names no file"), "{error}");
    assert!(!dir.join("new").exists());
    let error = ConfigFile::load(dir.join("sub").join("..")).unwrap_err();
    assert!(error.to_string().contains("names no file"), "{error}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_fifo_or_a_file_over_1_mib_is_refused_without_waiting() {
    let dir = temp_dir("fifo");
    let fifo = dir.join("fifo.toml");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    // No writer ever opens the FIFO, so a blocking open would hang this test.
    let error = ConfigFile::load(fifo).unwrap_err();
    assert_eq!(error.code(), crate::Code::Usage, "{error}");
    assert!(error.to_string().contains("not a regular file"), "{error}");

    let big = dir.join("big.toml");
    let mut text = "# padding\n".repeat(MAX_FILE_BYTES / 10);
    fs::write(&big, &text).unwrap();
    fs::set_permissions(&big, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(text.len(), MAX_FILE_BYTES - MAX_FILE_BYTES % 10);
    ConfigFile::load(big.clone()).unwrap();
    text.push_str(&"#".repeat(MAX_FILE_BYTES % 10 + 1));
    fs::write(&big, &text).unwrap();
    let error = ConfigFile::load(big.clone()).unwrap_err();
    assert_eq!(error.code(), crate::Code::Usage, "{error}");
    assert!(error.to_string().contains("larger than 1 MiB"), "{error}");
    assert!(error.hint().unwrap().contains("HIPPIUS_CONFIG"), "{error}");
    let error = store_token(&big).unwrap_err();
    assert!(error.to_string().contains("larger than 1 MiB"), "{error}");
    assert_eq!(fs::metadata(&big).unwrap().len(), MAX_FILE_BYTES as u64 + 1);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_lock_that_cannot_be_created_is_usage_and_nothing_is_written() {
    skip_if_root!();
    let dir = temp_dir("no-lock");
    // The owner may read and enter but not write: the directory check passes, and the
    // lock file cannot be created.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    let error = store_token(&dir.join("config.toml"));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let error = error.unwrap_err();
    assert_eq!(error.code(), crate::Code::Usage, "{error}");
    assert_eq!(
        error.to_string(),
        format!(
            "cannot lock {}: permission is denied",
            quote_path(&dir.join(".config.toml.lock"))
        )
    );
    assert_eq!(
        error.hint(),
        Some(&*format!("run `chmod 700 {}`", dir.display()))
    );
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
    fs::remove_dir_all(dir).unwrap();
}

/// A config the current user may not read is theirs to fix with a `chmod`, so it is
/// `usage`, as an unreadable `--token-file` is.
#[test]
fn a_config_file_without_read_permission_is_usage() {
    skip_if_root!();
    let dir = temp_dir("unreadable");
    let path = dir.join("config.toml");
    fs::write(&path, "").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let error = ConfigFile::load(path.clone()).unwrap_err();
    assert_eq!(error.code(), crate::Code::Usage, "{error}");
    assert_eq!(
        error.to_string(),
        format!("cannot read {}: permission is denied", quote_path(&path))
    );
    assert_eq!(
        error.hint(),
        Some(&*format!("run `chmod 600 {}`", path.display()))
    );
    fs::remove_dir_all(dir).unwrap();
}

/// A directory that cannot be created names the variable that moves the file, since no
/// `chmod` of a file `hipc` owns fixes it.
#[test]
fn a_config_directory_that_cannot_be_created_is_usage() {
    skip_if_root!();
    let dir = temp_dir("no-mkdir");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    let error = store_token(&dir.join("hipc").join("config.toml"));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let error = error.unwrap_err();
    assert_eq!(error.code(), crate::Code::Usage, "{error}");
    assert_eq!(
        error.to_string(),
        format!(
            "cannot create {}: permission is denied",
            quote_path(&dir.join("hipc"))
        )
    );
    assert_eq!(
        error.hint(),
        Some("set `HIPPIUS_CONFIG` to a file in a directory the current user can write")
    );
    fs::remove_dir_all(dir).unwrap();
}
