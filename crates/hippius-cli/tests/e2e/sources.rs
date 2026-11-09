//! Tenet 15, "Name the input, where it came from, and its value": a bad value from a
//! flag, a variable, a file, stdin, or the config file reads differently for each, with
//! the fix that belongs to it.

use std::os::unix::fs::PermissionsExt as _;

use serde_json::Value;

use crate::common::{Hipc, Home, TOKEN};

/// `hipc account get`, which fails before any request.
fn account_get(hipc: Hipc) -> Value {
    hipc.run(&["account", "get"]).fails(2, "usage")
}

#[test]
fn a_bad_token_names_where_it_came_from() {
    let home = Home::new();
    let error = account_get(Hipc::new(&home).env("HIPPIUS_TOKEN", "   "));
    assert_eq!(
        error["message"],
        "the token from `HIPPIUS_TOKEN` holds only whitespace"
    );
    assert_eq!(
        error["hint"],
        "unset `HIPPIUS_TOKEN`, or set it to a console token"
    );

    home.write_config("[profiles.work]\ntoken = \"tok a\"\n");
    let error = account_get(Hipc::without_api_url(&home).env("HIPPIUS_PROFILE", "work"));
    assert_eq!(
        error["message"],
        format!(
            "the token from the `token` of the profile `work` in `{}` holds a space, a line \
             break, or a character outside visible ASCII",
            home.config().display()
        )
    );
    assert_eq!(
        error["hint"],
        "run `hipc auth login --token-file <path>` to store a valid token"
    );

    let file = home.file("token", b"tok a");
    let error = Hipc::new(&home)
        .run(&["auth", "login", "--token-file", file.to_str().unwrap()])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        format!(
            "the token from `{}` holds a space, a line break, or a character outside visible \
             ASCII",
            file.display()
        )
    );
    assert_eq!(
        error["hint"],
        "leave only the console token, one word, in the file `--token-file` names"
    );
    let error = Hipc::new(&home)
        .stdin(b"")
        .run(&["auth", "login", "--token-file", "-"])
        .fails(2, "usage");
    assert_eq!(error["message"], "the token from stdin is empty");
    assert_eq!(
        error["hint"],
        "pipe only the console token, one word, to `--token-file -`"
    );
}

#[test]
fn a_bad_account_names_the_flag_the_variable_or_the_profile() {
    let home = Home::new();
    let tail = "is not an account id: an account is `self`, or `acct_` and 20 lowercase hex \
                characters";
    let error = account_get(
        Hipc::new(&home)
            .env("HIPPIUS_TOKEN", TOKEN)
            .env("HIPPIUS_ACCOUNT", "nope")
            .env("HIPPIUS_PROFILE", "default"),
    );
    assert_eq!(
        error["message"],
        format!("`nope` from `HIPPIUS_ACCOUNT` {tail}")
    );
    assert_eq!(
        error["hint"],
        "unset `HIPPIUS_ACCOUNT`, or set it to `self` or an id from `hipc account list`"
    );
    let error = Hipc::new(&home)
        .env("HIPPIUS_TOKEN", TOKEN)
        .env("HIPPIUS_ACCOUNT", "nope")
        .run(&["--account", "x", "account", "get"])
        .fails(2, "usage");
    assert_eq!(error["message"], format!("`x` from `--account` {tail}"));
    assert_eq!(
        error["hint"],
        "pass `--account self`, or an id from `hipc account list`"
    );

    home.write_config("[profiles.default]\naccount = \"nope\"\n");
    let error = account_get(Hipc::new(&home).env("HIPPIUS_TOKEN", TOKEN));
    assert_eq!(
        error["message"],
        format!(
            "`nope` from the `account` of the profile `default` in `{}` {tail}",
            home.config().display()
        )
    );
    assert_eq!(
        error["hint"],
        "run `hipc account use self` to clear the stored account"
    );
    // That hint works: the selection is rewritten without reading the bad one.
    Hipc::new(&home).run(&["account", "use", "self"]).ok();
    let error = Hipc::new(&home)
        .run(&["account", "use", "nope"])
        .fails(2, "usage");
    assert_eq!(error["message"], format!("`nope` {tail}"));
    assert_eq!(
        error["hint"],
        "run `hipc account list` to see the account ids the token can act in"
    );
}

#[test]
fn a_bad_origin_names_the_flag_the_variable_or_the_profile() {
    let home = Home::new();
    let tail = "is not an API origin: it has no scheme";
    let error = account_get(
        Hipc::new(&home)
            .env("HIPPIUS_TOKEN", TOKEN)
            .env("HIPPIUS_API_URL", "api.example"),
    );
    assert_eq!(
        error["message"],
        format!("`api.example` from `HIPPIUS_API_URL` {tail}")
    );
    assert_eq!(
        error["hint"],
        "unset `HIPPIUS_API_URL`, or set it to an origin such as `https://api.hippius.com`"
    );
    let error = Hipc::new(&home)
        .env("HIPPIUS_TOKEN", TOKEN)
        .run(&["--api-url", "api.example", "account", "get"])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        format!("`api.example` from `--api-url` {tail}")
    );
    assert_eq!(
        error["hint"],
        "pass an `--api-url` such as `https://api.hippius.com`"
    );

    home.write_config("[profiles.default]\napi_url = \"api.example\"\n");
    let error = account_get(Hipc::without_api_url(&home).env("HIPPIUS_TOKEN", TOKEN));
    let stored = format!(
        "the `api_url` of the profile `default` in `{}`",
        home.config().display()
    );
    assert_eq!(
        error["message"],
        format!("`api.example` from {stored} {tail}")
    );
    assert_eq!(error["hint"], format!("edit {stored}"));

    home.write_config("[profiles.default]\ntoken = \"tok\"\ntoken_origin = \"api.example\"\n");
    let error = account_get(Hipc::new(&home));
    assert_eq!(
        error["message"],
        format!(
            "`api.example` from the `token_origin` of the profile `default` in `{}` {tail}",
            home.config().display()
        )
    );
    assert_eq!(
        error["hint"],
        "run `hipc auth login --token-file <path>` to store the token again"
    );
}

/// The directory checks of `s3 configure` name the AWS file they protect and the variable
/// that moves it, not the config file and `HIPPIUS_CONFIG`.
#[test]
fn a_loose_aws_directory_names_the_aws_file() {
    let home = Home::new();
    let aws = home.path().join("aws");
    std::fs::create_dir_all(&aws).unwrap();
    std::fs::set_permissions(&aws, std::fs::Permissions::from_mode(0o777)).unwrap();
    let secret = home.file("secret", b"s3cr3t");
    let error = Hipc::new(&home)
        .env("AWS_CONFIG_FILE", aws.join("config").to_str().unwrap())
        .run(&[
            "s3",
            "configure",
            "--access-key-id",
            "hip_A1",
            "--secret-key-file",
            secret.to_str().unwrap(),
        ])
        .fails(2, "usage");
    std::fs::set_permissions(&aws, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        error["message"],
        format!(
            "the directory `{}` can be written by other users, so the AWS config file in it \
             cannot be trusted",
            aws.display()
        )
    );
}

/// A refused API path is quoted as the caller wrote it, without the `/api` prefix hipc adds.
#[test]
fn a_refused_path_is_quoted_as_written() {
    let home = Home::new();
    let error = Hipc::new(&home)
        .run(&["api", "GET", "/a/../b/?x=1"])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        "`/a/../b/` has an empty, `.`, or `..` segment, a percent-encoding, a backslash, a `;`, \
         or a character outside ASCII"
    );
}

/// An origin with a port and no host, or an IPv6 bracket that never closes, is refused as
/// `usage` before anything is sent, not left to fail inside the request.
#[test]
fn an_origin_without_a_host_is_a_usage_error() {
    let home = Home::new();
    for url in ["https://:443", "https://["] {
        let error = Hipc::new(&home)
            .env("HIPPIUS_TOKEN", TOKEN)
            .run(&["--api-url", url, "compute", "vms", "list"])
            .fails(2, "usage");
        assert_eq!(
            error["message"],
            format!("`{url}` from `--api-url` is not an API origin: it has no host")
        );
    }
}
