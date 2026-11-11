//! Snapshots of the `--dry-run` documents of a set of commands that covers each way a dry run is
//! built: one request, a read before the write with placeholders, a guard flag, a conditional
//! and an unconditional confirmation, a write that is not confirmed, `hipc api`, the selectors,
//! and a change to the local config. A dry run is wholly `hipc`'s, so the whole document is
//! the contract (tenet 5), and a change to any of them shows as one diff to review.
//!
//! Nothing is sent: the origin is the dead port of `common::no_server`, written as `[origin]`
//! in the snapshot, since its port differs on each run. Refresh a snapshot with
//! `INSTA_UPDATE=always cargo test -p hippius-cli --test e2e snapshots::`, then read the diff.

use crate::common::{self, Hipc, Home, PROJECT, TOKEN, VM};

/// The `--dry-run` document of `hipc <args> --dry-run`, pretty-printed, with the test origin
/// written as `[origin]`.
fn dry_run(args: &[&str]) -> String {
    let home = Home::new();
    let origin = common::no_server();
    let args = [args, &["--dry-run"]].concat();
    let document = Hipc::new(&home)
        .with_env_token(origin, TOKEN)
        .run(&args)
        .ok();
    serde_json::to_string_pretty(&document)
        .expect("a JSON value prints")
        .replace(origin, "[origin]")
}

/// Each command's dry run, as it prints it. The snapshot name is the command line.
#[test]
fn dry_run_documents() {
    let account = "acct_0123456789abcdef0123";
    let cases: [(&str, &[&str]); 14] = [
        ("vms-delete", &["compute", "vms", "delete", VM]),
        (
            "vms-create",
            &[
                "compute",
                "vms",
                "create",
                "--name",
                "web",
                "--flavor",
                "f1",
                "--image",
                "ubuntu",
                "--ssh-key-id",
                "7",
                "--region",
                "FR",
            ],
        ),
        ("vms-start", &["compute", "vms", "start", VM]),
        (
            "vms-delete-in-a-project",
            &["--project", PROJECT, "compute", "vms", "delete", VM],
        ),
        ("buckets-create", &["s3", "buckets", "create", "my-bucket"]),
        ("members-delete", &["account", "members", "delete", "42"]),
        (
            "projects-create",
            &["account", "projects", "create", "--name", "web"],
        ),
        (
            "auto-reload-update",
            &["billing", "auto-reload", "update", "--enabled", "true"],
        ),
        (
            "runners-limit-update",
            &[
                "compute",
                "runners",
                "limit",
                "update",
                "--monthly-cap",
                "10",
            ],
        ),
        (
            "zones-update-prefix",
            &[
                "cdn",
                "zones",
                "update",
                "zab12cd34ef56",
                "--prefix",
                "img/",
                "--public",
            ],
        ),
        (
            "hub-subscribe",
            &["hub", "subscription", "subscribe", "--plan", "pro"],
        ),
        ("network-create", &["network", "create"]),
        ("api-delete", &["api", "DELETE", "/api/compute/vms/vm_1/"]),
        ("account-use", &["account", "use", account]),
    ];
    for (name, args) in cases {
        insta::assert_snapshot!(name, dry_run(args));
    }
}
