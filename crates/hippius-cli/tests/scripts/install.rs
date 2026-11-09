//! The curl installer, `scripts/install.sh`, run with `/bin/sh` on a PATH of chosen tools
//! and a fake `uname`, against a local release server. The tarball holds the `hipc` this test
//! run built, so the installed binary runs.

use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::support::{Answer, TempDir, missing_tool, no_server, script, serve, sha256, tool};

const INSTALL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/install.sh");

/// The tools every run puts on PATH, besides a downloader and a hash tool. GNU tar runs
/// `gzip` for `-z`.
const BASE_TOOLS: [&str; 14] = [
    "awk", "cat", "chmod", "cp", "grep", "gzip", "mkdir", "mktemp", "mv", "rm", "sed", "tail",
    "tar", "tr",
];

/// The tag the server's releases/latest redirects to. It is not the version of the `hipc`
/// in the tarball, so a test can tell the two apart.
const LATEST: &str = "v9.8.7";

/// The version the `hipc` in every tarball prints.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The files besides `hipc` in each release tarball.
const LICENSES: [&str; 2] = ["LICENSE-MIT", "LICENSE-APACHE"];

/// A machine to install on: a HOME, and a PATH of only the tools the test chose.
struct Machine {
    home: TempDir,
    bin: PathBuf,
}

impl Machine {
    /// A machine whose `uname` reports `os` and `arch`, with `tools` on PATH.
    fn new(os: &str, arch: &str, tools: &[&str]) -> Self {
        let home = TempDir::new();
        let bin = home.path().join("tools");
        std::fs::create_dir(&bin).expect("create the tools directory");
        for name in BASE_TOOLS.iter().chain(tools) {
            symlink(tool(name), bin.join(name)).expect("link a tool");
        }
        let uname = format!("case \"$1\" in -s) echo {os} ;; -m) echo {arch} ;; esac");
        script(&bin, "uname", &uname);
        Self { home, bin }
    }

    /// Puts a fake tool named `name` on PATH, an `sh` script with `body`.
    fn fake(&self, name: &str, body: &str) {
        script(&self.bin, name, body);
    }

    /// Replaces one of [`BASE_TOOLS`] with a fake: the link to the real tool goes first, so
    /// the fake is written beside it and not through it.
    fn replace(&self, name: &str, body: &str) {
        std::fs::remove_file(self.bin.join(name)).expect("unlink the real tool");
        self.fake(name, body);
    }

    /// The default install directory, `~/.local/bin`.
    fn install_dir(&self) -> PathBuf {
        self.home.path().join(".local/bin")
    }

    /// The installer's TMPDIR, inside HOME, so it goes with the test's own temporary files.
    fn tmp(&self) -> PathBuf {
        self.home.path().join("tmp")
    }

    /// The installer with `args`, against the release server at `base`, not yet run. It
    /// runs under umask 077, so a directory it creates readable by others shows that it
    /// sets its own umask.
    fn installer(&self, base: &str, args: &[&str]) -> Command {
        // Without TMPDIR, `mktemp -d` falls back to /tmp, often a small tmpfs. Each run
        // downloads and unpacks a debug hipc of tens of MB there, so parallel tests filled
        // it and curl or tar failed mid-write ("Wrote only 512 of 10240 bytes").
        std::fs::create_dir_all(self.tmp()).expect("create the temp directory");
        let mut command = Command::new("/bin/sh");
        // The installed hipc runs here. Under `cargo llvm-cov` it is instrumented and, with
        // the environment cleared, writes its profile to the working directory.
        command
            .current_dir(self.home.path())
            .args(["-c", "umask 077 && exec /bin/sh \"$@\"", "sh", INSTALL])
            .args(args)
            .env_clear()
            .env("PATH", &self.bin)
            .env("HOME", self.home.path())
            .env("TMPDIR", self.tmp())
            .env("HIPC_INSTALL_BASE_URL", base)
            .env("HIPC_INSTALL_ALLOW_BASE_URL", "1");
        command
    }

    fn install(&self, base: &str, args: &[&str]) -> Output {
        self.installer(base, args)
            .output()
            .expect("run the installer")
    }

    /// A release tarball holding this test run's `hipc`, and its SHA-256.
    fn tarball(&self) -> (Vec<u8>, String) {
        self.tarball_of(Path::new(env!("CARGO_BIN_EXE_hipc")))
    }

    /// A gzipped tarball laid out as the release workflow packs it, and its SHA-256: the
    /// file `hipc`, named `hipc`, and the two license files, all at the top. Every test that
    /// installs checks that only `hipc` lands in the install directory.
    fn tarball_of(&self, hipc: &Path) -> (Vec<u8>, String) {
        let stage = self.home.path().join("stage");
        std::fs::create_dir(&stage).expect("create the staging directory");
        std::fs::copy(hipc, stage.join("hipc")).expect("copy hipc");
        for license in LICENSES {
            std::fs::write(stage.join(license), "license text").expect("write a license file");
        }
        let path = self.home.path().join("hipc.tar.gz");
        let out = Command::new(tool("tar"))
            .arg("-czf")
            .arg(&path)
            .arg("-C")
            .arg(&stage)
            .arg("hipc")
            .args(LICENSES)
            .output()
            .expect("run tar");
        assert!(
            out.status.success(),
            "tar failed: {:?}: {}",
            out.status,
            text(&out.stderr)
        );
        // The staged copy of hipc is as large as the binary; only the tarball is needed.
        std::fs::remove_dir_all(&stage).expect("remove the staging directory");
        let bytes = std::fs::read(&path).expect("read the tarball");
        (bytes, sha256(&path))
    }
}

/// The routes of release `tag` for `target`: its tarball and `sums` as its SHA256SUMS.
fn release(tag: &str, target: &str, tarball: &[u8], sums: &str) -> Vec<(String, Answer)> {
    vec![
        (
            format!("/download/{tag}/hipc-{target}.tar.gz"),
            Answer::File(tarball.to_vec()),
        ),
        (
            format!("/download/{tag}/SHA256SUMS"),
            Answer::File(sums.as_bytes().to_vec()),
        ),
    ]
}

/// The routes of releases/latest, redirecting to the page of [`LATEST`].
fn latest() -> Vec<(String, Answer)> {
    vec![
        ("/latest".to_owned(), Answer::Redirect("/tag/v9.8.7")),
        (
            format!("/tag/{LATEST}"),
            Answer::File(b"release page".to_vec()),
        ),
    ]
}

/// A SHA256SUMS file with `sum` for `target`, between the line of another target and the
/// line of `install.sh`, as a release lists it.
fn sums(sum: &str, target: &str) -> String {
    format!(
        "{other}  hipc-other.tar.gz\n{sum}  hipc-{target}.tar.gz\n{other}  install.sh\n",
        other = "0".repeat(64)
    )
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("the installer writes UTF-8")
}

/// The names in directory `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read the install directory")
        .map(|entry| {
            let entry = entry.expect("a directory entry");
            entry.file_name().into_string().expect("a UTF-8 name")
        })
        .collect();
    names.sort();
    names
}

#[test]
fn the_latest_release_installs_to_local_bin_and_prints_the_path_line() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    // Another hipc on PATH, as `cargo install` leaves one; it keeps running after this.
    machine.fake("hipc", "echo hipc 0.0.1");
    let target = "x86_64-unknown-linux-musl";
    let (tarball, sum) = machine.tarball();
    let mut routes = latest();
    routes.extend(release(LATEST, target, &tarball, &sums(&sum, target)));

    let base = serve(routes);

    let out = machine.install(&base, &[]);

    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(out.stdout.is_empty(), "the installer writes only to stderr");
    let local_bin = machine.install_dir();
    let installing = format!(
        "installing hipc {LATEST} for {target} to {} from {base}\n",
        local_bin.display()
    );
    assert!(stderr.contains(&installing), "{stderr}");
    assert!(
        stderr.contains(&format!("installed hipc {VERSION} to ")),
        "{stderr}"
    );
    assert!(
        stderr.contains("note: signature not checked: cosign is not on PATH"),
        "{stderr}"
    );
    assert!(stderr.contains("is not on PATH; add this line"), "{stderr}");
    let shadowing = format!(
        "note: {} comes first on PATH and runs instead of {}/hipc\n",
        machine.bin.join("hipc").display(),
        local_bin.display()
    );
    assert!(stderr.contains(&shadowing), "{stderr}");
    let dir = machine.install_dir();
    // The temporary copy was renamed, not left beside the binary.
    assert_eq!(entries(&dir), ["hipc"]);
    let mode = std::fs::metadata(dir.join("hipc"))
        .expect("hipc")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    // Created under umask 077, readable by everyone all the same.
    let mode = std::fs::metadata(&local_bin)
        .expect("the install directory")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
}

#[test]
fn a_pinned_version_installs_that_release_without_asking_for_the_latest() {
    let machine = Machine::new("Linux", "aarch64", &["curl", "sha256sum"]);
    let target = "aarch64-unknown-linux-musl";
    let (tarball, sum) = machine.tarball();
    // No releases/latest route: asking for it would fail the run with a 404.
    let base = serve(release("v0.12.0", target, &tarball, &sums(&sum, target)));
    let dir = machine.home.path().join("custom bin");
    // The PATH entry without a slash, with a trailing one, and with a doubled one inside (a
    // `$HOME//.local/bin` from a HOME written with a slash), as profiles write it.
    let paths = [
        format!("{}:{}", machine.bin.display(), dir.display()),
        format!("{}:{}/", machine.bin.display(), dir.display()),
        format!(
            "{}:{}//custom bin",
            machine.bin.display(),
            machine.home.path().display()
        ),
    ];

    for (i, args) in [
        &["0.12.0"][..],
        &["v0.12.0"],
        &["--version", "0.12.0"],
        &["--version=v0.12.0"],
    ]
    .into_iter()
    .enumerate()
    {
        let out = machine
            .installer(&base, args)
            // Trailing slashes still match the PATH entry written without them.
            .env("HIPC_INSTALL_DIR", format!("{}//", dir.display()))
            .env("PATH", &paths[i % 3])
            .output()
            .expect("run the installer");

        let stderr = text(&out.stderr);
        assert!(out.status.success(), "{args:?}: {stderr}");
        assert!(
            stderr.contains(&format!("installing hipc v0.12.0 for {target}")),
            "{stderr}"
        );
        assert!(!stderr.contains("bin is not on PATH"), "{args:?}: {stderr}");
        // The hipc on PATH is the one just installed, so nothing shadows it.
        assert!(
            !stderr.contains("comes first on PATH"),
            "{args:?}: {stderr}"
        );
        // Each run replaced the previous binary in place.
        assert_eq!(entries(&dir), ["hipc"], "{args:?}");
    }
}

#[test]
fn a_home_with_a_trailing_slash_gives_a_clean_install_path() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    let target = "x86_64-unknown-linux-musl";
    let (tarball, sum) = machine.tarball();
    let base = serve(release("v1.0.0", target, &tarball, &sums(&sum, target)));

    let out = machine
        .installer(&base, &["1.0.0"])
        .env("HOME", format!("{}/", machine.home.path().display()))
        .output()
        .expect("run the installer");

    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let installed = format!(
        "installed hipc {VERSION} to {}/hipc\n",
        machine.install_dir().display()
    );
    assert!(stderr.contains(&installed), "{stderr}");
    // The doubled slash of HOME/ + /.local/bin is gone; the base URL keeps its own.
    assert!(!stderr.contains("//."), "{stderr}");
}

#[test]
fn a_relative_or_tilde_install_dir_is_refused_before_any_download() {
    for (var, dir, message) in [
        (
            "HIPC_INSTALL_DIR",
            "relbin",
            "HIPC_INSTALL_DIR must be an absolute path, not relbin",
        ),
        (
            "HIPC_INSTALL_DIR",
            "./bin",
            "HIPC_INSTALL_DIR must be an absolute path, not ./bin",
        ),
        (
            "HIPC_INSTALL_DIR",
            "~/tilde",
            "HIPC_INSTALL_DIR must not start with a literal ~",
        ),
        // The default directory is under HOME, so a relative HOME is the same problem.
        (
            "HOME",
            "relhome",
            "HOME must be an absolute path, not relhome",
        ),
        ("HOME", "~", "HOME must not start with a literal ~"),
    ] {
        let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);

        let out = machine
            .installer(no_server(), &[])
            .env(var, dir)
            .output()
            .expect("run the installer");

        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{var}={dir}: {stderr}");
        assert!(stderr.contains(message), "{var}={dir}: {stderr}");
        assert!(!stderr.contains("installing"), "{var}={dir}: {stderr}");
        // Nothing was made under the working directory either.
        assert!(!machine.home.path().join(dir).exists(), "{var}={dir}");
        assert!(!machine.home.path().join("~").exists(), "{var}={dir}");
    }
}

#[test]
fn a_directory_named_hip_in_the_install_dir_is_refused() {
    // One there from the start is refused before any download, against a dead server; one
    // that appears during the run (a fake tar makes it) is refused before the copy.
    for early in [true, false] {
        let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
        let dir = machine.install_dir();
        let target = "x86_64-unknown-linux-musl";
        let (tarball, sum) = machine.tarball();
        let base = if early {
            std::fs::create_dir_all(dir.join("hipc")).expect("create a directory named hipc");
            no_server().to_owned()
        } else {
            machine.replace(
                "tar",
                &format!(
                    "mkdir -p '{}' && exec '{}' \"$@\"",
                    dir.join("hipc").display(),
                    tool("tar").display()
                ),
            );
            serve(release("v1.0.0", target, &tarball, &sums(&sum, target)))
        };

        let out = machine.install(&base, &["1.0.0"]);

        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{early}: {stderr}");
        assert!(
            stderr.contains(&format!(
                "{}/hipc is a directory; remove or rename it first; nothing was installed",
                dir.display()
            )),
            "{early}: {stderr}"
        );
        assert_eq!(stderr.contains("installing hipc"), !early, "{stderr}");
        assert!(!stderr.contains("installed hipc"), "{early}: {stderr}");
        // mv would have moved the copy into it under its temporary name.
        assert_eq!(entries(&dir), ["hipc"], "{early}");
        assert_eq!(entries(&dir.join("hipc")), Vec::<String>::new(), "{early}");
    }
}

/// A signal routed through `exit` runs the cleanup trap and exits with 128 plus the signal,
/// so a caller can tell an interruption from a failure.
#[test]
fn a_signal_cleans_up_and_exits_with_its_number() {
    for (signal, code) in [("HUP", 129), ("INT", 130), ("TERM", 143)] {
        let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
        // tar runs from main itself, so its parent is the installer.
        machine.replace("tar", &format!("kill -{signal} $PPID"));
        let target = "x86_64-unknown-linux-musl";
        let (tarball, sum) = machine.tarball();
        let base = serve(release("v1.0.0", target, &tarball, &sums(&sum, target)));
        let out = machine.install(&base, &["1.0.0"]);

        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(code), "{signal}: {stderr}");
        assert!(!stderr.contains("installed hipc"), "{signal}: {stderr}");
        assert!(!machine.install_dir().exists(), "{signal}");
        // The downloads were under TMPDIR; the EXIT trap removed them.
        assert_eq!(entries(&machine.tmp()), Vec::<String>::new(), "{signal}");
    }
}

#[test]
fn a_tarball_that_does_not_match_sha256sums_installs_nothing() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    let target = "x86_64-unknown-linux-musl";
    let (tarball, _) = machine.tarball();
    let dir = machine.install_dir();
    std::fs::create_dir_all(&dir).expect("create the install directory");
    std::fs::write(dir.join("hipc"), "old").expect("write an old hipc");

    for (sums, message) in [
        (sums(&"f".repeat(64), target), "does not match SHA256SUMS"),
        (
            sums(&"f".repeat(63), target),
            "SHA256SUMS has no single well-formed line",
        ),
        (
            sums(&"F".repeat(64), target),
            "SHA256SUMS has no single well-formed line",
        ),
        (
            sums(&"0".repeat(64), "other"),
            "SHA256SUMS has no single well-formed line",
        ),
    ] {
        let base = serve(release("v1.0.0", target, &tarball, &sums));

        let out = machine.install(&base, &["1.0.0"]);

        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains(message), "{stderr}");
        assert_eq!(std::fs::read(dir.join("hipc")).expect("hipc"), b"old");
        assert_eq!(entries(&dir), ["hipc"]);
    }
}

/// A hash-valid tarball whose `hipc` member is a symlink to a file on this machine, alone or
/// after a regular `hipc` (tar extracts the last one), installs nothing and runs nothing:
/// the link would make the installer copy and run whatever it points at.
#[test]
fn a_tarball_whose_hipc_is_a_symlink_installs_nothing() {
    for duplicate in [false, true] {
        let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
        let target = "x86_64-unknown-linux-musl";
        let ran = machine.home.path().join("ran");
        // A short name: the link target must fit ustar's 100 bytes (see `TempDir::new`).
        let local = machine.home.path().join("prog");
        std::fs::write(
            &local,
            format!("#!/bin/sh\ntouch '{}'\necho hipc 0.0.0\n", ran.display()),
        )
        .expect("write the local program");
        std::fs::set_permissions(&local, std::fs::Permissions::from_mode(0o755))
            .expect("make it executable");
        let stage = machine.home.path().join("stage");
        std::fs::create_dir(&stage).expect("create the staging directory");
        let archive = machine.home.path().join("hipc.tar");
        let tar = |args: &[&str]| {
            let out = Command::new(tool("tar"))
                .args(args)
                .current_dir(&stage)
                .output()
                .expect("run tar");
            assert!(out.status.success(), "tar: {}", text(&out.stderr));
        };
        let archive_arg = archive.to_str().expect("UTF-8");
        if duplicate {
            std::fs::write(stage.join("hipc"), "regular").expect("write a regular hipc");
            tar(&["-cf", archive_arg, "hipc"]);
            std::fs::remove_file(stage.join("hipc")).expect("remove it");
            symlink(&local, stage.join("hipc")).expect("link hipc");
            tar(&["-rf", archive_arg, "hipc"]);
        } else {
            symlink(&local, stage.join("hipc")).expect("link hipc");
            tar(&["-cf", archive_arg, "hipc"]);
        }
        let gzip = Command::new(tool("gzip"))
            .arg("-f")
            .arg(&archive)
            .output()
            .expect("run gzip");
        assert!(gzip.status.success(), "gzip: {}", text(&gzip.stderr));
        let gzipped = machine.home.path().join("hipc.tar.gz");
        let tarball = std::fs::read(&gzipped).expect("read the tarball");
        let base = serve(release(
            "v1.0.0",
            target,
            &tarball,
            &sums(&sha256(&gzipped), target),
        ));

        let out = machine.install(&base, &["1.0.0"]);

        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{duplicate}: {stderr}");
        assert!(
            stderr.contains(&format!(
                "the hipc in hipc-{target}.tar.gz is not a regular file; nothing was installed"
            )),
            "{duplicate}: {stderr}"
        );
        assert!(!ran.exists(), "{duplicate}: the linked program ran");
        assert!(!machine.install_dir().exists(), "{duplicate}");
    }
}

/// A download refused for a reason other than a 404 (here a file larger than any release
/// tarball, by its `Content-Length`) names curl's reason, not a missing release.
#[test]
fn a_download_that_fails_for_another_reason_than_a_404_does_not_blame_the_release() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    let target = "x86_64-unknown-linux-musl";
    let base = serve(vec![(
        format!("/download/v1.0.0/hipc-{target}.tar.gz"),
        Answer::Huge(300 * 1024 * 1024),
    )]);

    let out = machine.install(&base, &["1.0.0"]);

    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "cannot download {base}/download/v1.0.0/hipc-{target}.tar.gz; see the reason above"
        )),
        "{stderr}"
    );
    assert!(stderr.contains("curl: (63)"), "{stderr}");
    assert!(!stderr.contains("no release"), "{stderr}");
    assert!(!machine.install_dir().exists());
}

/// A missing SHA256SUMS, a 404, says the release has none.
#[test]
fn a_release_without_sha256sums_says_so() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    let target = "x86_64-unknown-linux-musl";
    let (tarball, _) = machine.tarball();
    let base = serve(vec![(
        format!("/download/v1.0.0/hipc-{target}.tar.gz"),
        Answer::File(tarball),
    )]);

    let out = machine.install(&base, &["1.0.0"]);

    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("/download/v1.0.0/SHA256SUMS (release v1.0.0 has no SHA256SUMS)"),
        "{stderr}"
    );
}

#[test]
fn an_unsupported_machine_is_refused_before_any_download() {
    for (os, arch, message) in [
        (
            "Linux",
            "armv7l",
            "no hipc build for the armv7l architecture",
        ),
        ("Linux", "i686", "no hipc build for the i686 architecture"),
        ("FreeBSD", "amd64", "no hipc build for FreeBSD"),
        (
            "MINGW64_NT-10.0",
            "x86_64",
            "no hipc build for Windows (MINGW64_NT-10.0); run this under WSL",
        ),
        (
            "MSYS_NT-10.0",
            "x86_64",
            "no hipc build for Windows (MSYS_NT-10.0); run this under WSL",
        ),
        (
            "CYGWIN_NT-10.0",
            "x86_64",
            "no hipc build for Windows (CYGWIN_NT-10.0); run this under WSL",
        ),
    ] {
        let machine = Machine::new(os, arch, &["curl", "sha256sum"]);

        let out = machine.install(no_server(), &[]);

        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{os} {arch}: {stderr}");
        assert!(stderr.contains(message), "{stderr}");
        assert!(!machine.install_dir().exists());
    }
}

/// A shell under Rosetta reports `x86_64`; `sysctl.proc_translated` says it is translated.
#[test]
fn macos_installs_the_arm_build_on_apple_silicon_also_under_rosetta() {
    for (arch, translated, target) in [
        ("arm64", "", "aarch64-apple-darwin"),
        ("x86_64", "1", "aarch64-apple-darwin"),
        ("x86_64", "0", "x86_64-apple-darwin"),
    ] {
        let machine = Machine::new("Darwin", arch, &["curl", "sha256sum"]);
        machine.fake("sysctl", &format!("echo {translated}"));
        let (tarball, sum) = machine.tarball();
        let base = serve(release("v1.0.0", target, &tarball, &sums(&sum, target)));

        let out = machine.install(&base, &["1.0.0"]);

        let stderr = text(&out.stderr);
        assert!(out.status.success(), "{arch} {translated}: {stderr}");
        assert!(stderr.contains(&format!("for {target} to")), "{stderr}");
    }
}

/// A machine with a fake `cosign` of version `version` that logs its arguments and the
/// bundle it was given, and exits with `status`; and a release whose SHA256SUMS has
/// `bundle` beside it when given. The signature is required, as a pipeline would ask.
fn signed(version: &str, status: u8, bundle: Option<&str>) -> (Machine, Output) {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    let log = machine.home.path().join("cosign.log");
    // The banner of `cosign version`, in the layout of cosign 1, 2 and 3.
    let body = format!(
        "case \"$1\" in version) printf 'GitVersion:    {version}\\nGitCommit: 0\\n'; \
         exit 0 ;; esac\n\
         echo \"$@\" > '{log}'\ncat \"$3\" >> '{log}'\n\
         echo 'cosign: certificate does not match' >&2\nexit {status}",
        log = log.display()
    );
    machine.fake("cosign", &body);
    let target = "x86_64-unknown-linux-musl";
    let (tarball, sum) = machine.tarball();
    let mut routes = release("v1.0.0", target, &tarball, &sums(&sum, target));
    if let Some(bundle) = bundle {
        let path = "/download/v1.0.0/SHA256SUMS.sigstore.json".to_owned();
        routes.push((path, Answer::File(bundle.as_bytes().to_vec())));
    }

    let out = machine
        .installer(&serve(routes), &["1.0.0"])
        .env("HIPC_INSTALL_REQUIRE_SIGNATURE", "1")
        .output()
        .expect("run the installer");
    (machine, out)
}

#[test]
fn a_cosign_before_3_is_refused_since_it_cannot_read_the_bundle() {
    // The release signs with cosign 3; 1.x has no --bundle, and 2.x reads the bundle only
    // from 2.4 with a switch the script does not pass.
    for (version, major) in [("v1.13.1", "1"), ("v2.4.1", "2")] {
        let (machine, out) = signed(version, 0, Some("the bundle"));

        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{version}: {stderr}");
        assert!(
            stderr.contains(&format!(
                "cosign 3.0 or later is needed to check the Sigstore bundle; \
                 this is cosign {major}.x; nothing was installed"
            )),
            "{version}: {stderr}"
        );
        // verify-blob never ran: the log only a verification writes is absent.
        assert!(
            !machine.home.path().join("cosign.log").exists(),
            "{version}"
        );
        assert!(!machine.install_dir().exists(), "{version}");
    }
}

#[test]
fn a_cosign_banner_without_a_version_is_left_to_verify_blob() {
    let (_, out) = signed("unknown", 0, Some("the bundle"));

    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("SHA256SUMS signature verified."),
        "{stderr}"
    );
    // No `[: Illegal number` from comparing the word: every line is the installer's own.
    assert!(
        stderr
            .lines()
            .all(|line| line.starts_with("hipc-install: ")),
        "{stderr}"
    );
}

#[test]
fn a_required_signature_fails_the_install_without_cosign() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    let target = "x86_64-unknown-linux-musl";
    let (tarball, sum) = machine.tarball();
    let base = serve(release("v1.0.0", target, &tarball, &sums(&sum, target)));

    let out = machine
        .installer(&base, &["1.0.0"])
        .env("HIPC_INSTALL_REQUIRE_SIGNATURE", "1")
        .output()
        .expect("run the installer");

    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "cosign is not on PATH and HIPC_INSTALL_REQUIRE_SIGNATURE=1 is set; \
             nothing was installed"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("note: signature not checked"), "{stderr}");
    assert!(!machine.install_dir().exists());

    // Only the value 1 requires it.
    let out = machine
        .installer(&base, &["1.0.0"])
        .env("HIPC_INSTALL_REQUIRE_SIGNATURE", "yes")
        .output()
        .expect("run the installer");

    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("note: signature not checked"), "{stderr}");
}

#[test]
fn cosign_on_path_checks_the_signature_against_the_release_workflow() {
    let (machine, out) = signed("v3.0.6", 0, Some("the bundle"));

    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("SHA256SUMS signature verified."),
        "{stderr}"
    );
    assert!(!stderr.contains("not checked"), "{stderr}");
    assert!(
        !stderr.contains("cosign: certificate"),
        "a passing check prints nothing of cosign"
    );
    let log = std::fs::read_to_string(machine.home.path().join("cosign.log")).expect("log");
    let (args, bundle) = log
        .split_once('\n')
        .expect("the arguments, then the bundle");
    assert!(args.starts_with("verify-blob --bundle "), "{args}");
    // The release workflow of thenervelab/hippius-cli, run for exactly the tag installed.
    let identity = concat!(
        r"^https://github\.com/thenervelab/hippius-cli/",
        r"\.github/workflows/release\.yml@refs/tags/v1\.0\.0$",
    );
    let expected = format!(
        " --certificate-identity-regexp {identity} \
         --certificate-oidc-issuer https://token.actions.githubusercontent.com "
    );
    assert!(args.contains(&expected), "{args}");
    assert!(args.ends_with("/SHA256SUMS"), "{args}");
    assert_eq!(bundle, "the bundle");
}

#[test]
fn a_signature_that_does_not_verify_installs_nothing() {
    let (machine, out) = signed("v3.1.0", 1, Some("a forged bundle"));

    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("the signature of SHA256SUMS does not verify"),
        "{stderr}"
    );
    assert!(
        stderr.contains("cosign: certificate does not match"),
        "{stderr}"
    );
    assert!(!machine.install_dir().exists());
}

#[test]
fn a_release_without_a_signature_is_refused_when_cosign_is_on_path() {
    let (machine, out) = signed("v3.0.6", 0, None);

    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("cannot download http://"), "{stderr}");
    assert!(
        stderr.contains("/download/v1.0.0/SHA256SUMS.sigstore.json"),
        "{stderr}"
    );
    assert!(!machine.install_dir().exists());
}

#[test]
fn wget_and_shasum_stand_in_for_curl_and_sha256sum() {
    if missing_tool(&["wget", "shasum"]).is_some() {
        return;
    }
    let machine = Machine::new("Linux", "x86_64", &["wget", "shasum"]);
    let target = "x86_64-unknown-linux-musl";
    let (tarball, sum) = machine.tarball();
    let mut routes = latest();
    routes.extend(release(LATEST, target, &tarball, &sums(&sum, target)));

    let out = machine.install(&serve(routes), &[]);

    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains(&format!("installing hipc {LATEST} for {target}")),
        "{stderr}"
    );
    assert_eq!(entries(&machine.install_dir()), ["hipc"]);
}

#[test]
fn bad_arguments_and_a_missing_tool_or_release_are_refused() {
    let no_routes = serve(Vec::new());
    let nightly = serve(vec![
        ("/latest".to_owned(), Answer::Redirect("/tag/nightly")),
        ("/tag/nightly".to_owned(), Answer::File(b"page".to_vec())),
    ]);
    let curl = &["curl", "sha256sum"][..];
    for (tools, base, args, message) in [
        (
            curl,
            no_server(),
            &["--bogus"][..],
            "unknown option: --bogus",
        ),
        (curl, no_server(), &["--version"], "--version needs a value"),
        (curl, no_server(), &["1..2"], "not a version: 1..2"),
        (curl, no_server(), &["latest"], "not a version: latest"),
        (
            curl,
            no_server(),
            &["1.0.0/../x"],
            "not a version: 1.0.0/../x",
        ),
        (
            &["sha256sum"],
            no_server(),
            &[],
            "neither curl nor wget is on PATH",
        ),
        (
            &["curl"],
            no_server(),
            &[],
            "neither sha256sum nor shasum is on PATH",
        ),
        (curl, &no_routes, &[], "cannot reach http://"),
        (
            &["wget", "sha256sum"],
            &no_routes,
            &[],
            "cannot find the latest release",
        ),
        (curl, &nightly, &[], "not a version: nightly"),
        (
            curl,
            &no_routes,
            &["1.0.0"],
            "(no release v1.0.0, or no build for x86_64-unknown-linux-musl in it)",
        ),
    ] {
        // A stock Mac has no `wget`: that row is skipped there, and runs on CI's images.
        if missing_tool(tools).is_some() {
            continue;
        }
        let machine = Machine::new("Linux", "x86_64", tools);

        let out = machine.install(base, args);

        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(stderr.contains(message), "{args:?}: {stderr}");
        assert!(!machine.install_dir().exists());
    }
}

#[test]
fn help_exits_zero_and_home_is_needed_only_without_hip_install_dir() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);

    let help = machine.install(no_server(), &["--help"]);
    assert!(help.status.success());
    let usage = text(&help.stderr);
    assert!(usage.starts_with("usage: install.sh [VERSION]"), "{usage}");
    assert!(
        usage.contains("HIPC_INSTALL_REQUIRE_SIGNATURE=1 fails the install"),
        "{usage}"
    );
    assert!(
        usage.contains("Uninstall: remove $HIPC_INSTALL_DIR/hipc"),
        "{usage}"
    );

    let out = machine
        .installer(no_server(), &[])
        .env_remove("HOME")
        .output()
        .expect("run the installer");
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("HOME is not set; set HIPC_INSTALL_DIR"),
        "{stderr}"
    );
}

/// A download cut short at any line runs nothing: every line before the last only defines
/// a function or sets a variable, and `main "$@"` is the last line.
#[test]
fn a_partial_download_installs_nothing() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    let script = std::fs::read(INSTALL).expect("read the installer");
    let partial = machine.home.path().join("partial.sh");
    let ends: Vec<usize> = script
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .map(|(i, _)| i + 1)
        .collect();
    assert!(ends.len() > 100, "the installer has its lines");

    // Every prefix that ends at a line break, except the whole script.
    for end in &ends[..ends.len() - 1] {
        std::fs::write(&partial, &script[..*end]).expect("write the partial script");

        let out = Command::new("/bin/sh")
            .arg(&partial)
            .env_clear()
            .env("PATH", &machine.bin)
            .env("HOME", machine.home.path())
            .env("HIPC_INSTALL_BASE_URL", no_server())
            .output()
            .expect("run the partial installer");

        assert!(out.stdout.is_empty(), "cut at byte {end}");
        assert!(!machine.install_dir().exists(), "cut at byte {end}");
        assert!(
            !text(&out.stderr).contains("installing"),
            "cut at byte {end}"
        );
    }
}

#[test]
fn a_hip_that_does_not_run_leaves_the_installed_one_in_place() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    let target = "x86_64-unknown-linux-musl";
    script(machine.home.path(), "broken-hipc", "exit 126");
    let (tarball, sum) = machine.tarball_of(&machine.home.path().join("broken-hipc"));
    let base = serve(release("v1.0.0", target, &tarball, &sums(&sum, target)));
    let dir = machine.install_dir();
    std::fs::create_dir_all(&dir).expect("create the install directory");
    std::fs::write(dir.join("hipc"), "old").expect("write an old hipc");

    let out = machine.install(&base, &["1.0.0"]);

    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("the downloaded hipc does not run on this machine"),
        "{stderr}"
    );
    assert_eq!(std::fs::read(dir.join("hipc")).expect("hipc"), b"old");
    // The temporary copy that failed to run is gone too.
    assert_eq!(entries(&dir), ["hipc"]);
}

#[test]
fn an_inherited_base_url_is_ignored_without_the_second_variable() {
    let machine = Machine::new("Linux", "x86_64", &["sha256sum"]);
    let log = machine.home.path().join("curl.log");
    machine.fake(
        "curl",
        &format!("echo \"$@\" > '{}'\nexit 22", log.display()),
    );

    let out = machine
        .installer(no_server(), &[])
        .env_remove("HIPC_INSTALL_ALLOW_BASE_URL")
        .output()
        .expect("run the installer");

    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("note: ignoring HIPC_INSTALL_BASE_URL"),
        "{stderr}"
    );
    let releases = "https://github.com/thenervelab/hippius-cli/releases";
    assert!(
        stderr.contains(&format!("cannot reach {releases}/latest")),
        "{stderr}"
    );
    // -q comes first, so curl reads no ~/.curlrc; only HTTPS, TLS 1.2 or later; three
    // retries of a transient failure and a bounded connect.
    let args = std::fs::read_to_string(&log).expect("the curl log");
    // A download is bounded in time, in speed and in size.
    assert!(
        args.starts_with(
            "-q --proto =https --tlsv1.2 -fsSL --retry 3 --retry-delay 1 --connect-timeout 15 \
             --max-time 900 --speed-limit 1024 --speed-time 30 --max-filesize 209715200 "
        ),
        "{args}"
    );
    // The latest tag comes from the redirect alone: a HEAD request, no release page.
    assert!(
        args.trim_end().ends_with(&format!(
            "-I -o /dev/null -w %{{url_effective}} {releases}/latest"
        )),
        "{args}"
    );
}

/// A fake GNU wget that fails every download as a 404, with the headers indented, a blank
/// line, and its own message unindented, as wget -S writes them.
const FAILING_WGET: &str = r#"case "$*" in *--help*) echo '--no-config --https-only'; exit 0 ;; esac
echo "$@" >> "$HOME/wget.log"
echo '  HTTP/1.1 404 Not Found' >&2
echo '  Content-Length: 9' >&2
echo '' >&2
echo 'wget: server returned error: HTTP/1.1 404 Not Found' >&2
exit 8"#;

#[test]
fn a_failed_wget_download_shows_the_reason_wget_gave() {
    // A pinned release's tarball, and the latest release's redirect.
    for (args, message) in [
        (
            &["1.0.0"][..],
            "(no release v1.0.0, or no build for x86_64-unknown-linux-musl in it)",
        ),
        (
            &[],
            "cannot find the latest release at https://releases.invalid/latest",
        ),
    ] {
        let machine = Machine::new("Linux", "x86_64", &["sha256sum"]);
        machine.fake("wget", FAILING_WGET);

        let out = machine.install("https://releases.invalid", args);

        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(
            stderr.contains("wget: server returned error: HTTP/1.1 404 Not Found\n"),
            "{args:?}: {stderr}"
        );
        // Neither the headers nor the blank line before wget's message.
        assert!(!stderr.contains("Content-Length"), "{args:?}: {stderr}");
        assert!(!stderr.contains("\n\n"), "{args:?}: {stderr}");
        assert!(stderr.contains(message), "{args:?}: {stderr}");
        assert!(!machine.install_dir().exists(), "{args:?}");
        let log = std::fs::read_to_string(machine.home.path().join("wget.log")).expect("log");
        assert!(
            log.starts_with("--no-config --https-only --tries=3 --timeout=15 -S "),
            "{log}"
        );
    }
}

/// A fake GNU wget: `--help` names both switches the installer needs; every other run logs
/// its arguments, writes `payload` to its `-O` file, and reports a redirect to plain HTTP.
const DOWNGRADING_WGET: &str = r#"case "$*" in *--help*) echo '--no-config --https-only'; exit 0 ;; esac
echo "$@" >> "$HOME/wget.log"
out=
while [ $# -gt 0 ]; do
  if [ "$1" = -O ]; then out=$2; fi
  shift
done
echo '  Location: http://evil.example/hipc.tar.gz' >&2
if [ "$out" != /dev/null ]; then echo payload > "$out"; fi"#;

#[test]
fn wget_refuses_a_redirect_from_https_to_plain_http() {
    for args in [&[][..], &["1.0.0"]] {
        let machine = Machine::new("Linux", "x86_64", &["sha256sum"]);
        machine.fake("wget", DOWNGRADING_WGET);

        let out = machine.install("https://releases.invalid", args);

        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(
            stderr.contains("refused a redirect from HTTPS to plain HTTP"),
            "{stderr}"
        );
        assert!(!machine.install_dir().exists());
        let log = std::fs::read_to_string(machine.home.path().join("wget.log")).expect("log");
        assert!(
            log.starts_with("--no-config --https-only --tries=3 --timeout=15 "),
            "{log}"
        );
    }
}

#[test]
fn a_wget_without_the_needed_switches_is_refused() {
    let machine = Machine::new("Linux", "x86_64", &["sha256sum"]);
    machine.fake("wget", "echo 'BusyBox v1.36.1 multi-call binary.'");

    let out = machine.install(no_server(), &[]);

    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("this wget has no --no-config (BusyBox?)"),
        "{stderr}"
    );
}

/// An install run by a pipeline written for 0.14.0, whose one documented variable was
/// `HIP_INSTALL_DIR`: the old name is used when the new one is unset and the directory holds
/// the `hip` that release installed, with a note, and the directory asked for is the one
/// installed to.
#[test]
fn an_old_install_dir_is_used_when_the_new_one_is_unset() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    let target = "x86_64-unknown-linux-musl";
    let (tarball, sum) = machine.tarball();
    let base = serve(release("v1.0.0", target, &tarball, &sums(&sum, target)));
    let dir = machine.home.path().join("old bin");
    std::fs::create_dir(&dir).expect("create the old install directory");
    std::fs::write(dir.join("hip"), "the 0.14.0 binary").expect("write the old hip");

    let out = machine
        .installer(&base, &["1.0.0"])
        .env("HIP_INSTALL_DIR", &dir)
        .output()
        .expect("run the installer");

    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("note: HIP_INSTALL_DIR is now HIPC_INSTALL_DIR; using HIP_INSTALL_DIR\n"),
        "{stderr}"
    );
    assert_eq!(entries(&dir), ["hip", "hipc"]);
    assert!(!machine.install_dir().exists());
}

/// `HIP_` is a common prefix, so an old-name directory with no `hip` in it, which no 0.14.0
/// install made, is not used: the install goes to the default directory, with a note.
#[test]
fn an_old_install_dir_without_a_hip_is_ignored() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    let target = "x86_64-unknown-linux-musl";
    let (tarball, sum) = machine.tarball();
    let base = serve(release("v1.0.0", target, &tarball, &sums(&sum, target)));
    let other = machine.home.path().join("another tool");
    std::fs::create_dir(&other).expect("create another tool's directory");

    let out = machine
        .installer(&base, &["1.0.0"])
        .env("HIP_INSTALL_DIR", &other)
        .output()
        .expect("run the installer");

    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains(
            "note: ignoring HIP_INSTALL_DIR: it holds no hip from an earlier install; \
             set HIPC_INSTALL_DIR to choose where hipc goes\n"
        ),
        "{stderr}"
    );
    assert_eq!(entries(&other), Vec::<String>::new());
    assert_eq!(entries(&machine.install_dir()), ["hipc"]);
}

/// A bad old-name directory is reported under the name it was given by.
#[test]
fn a_relative_old_install_dir_is_refused_by_its_own_name() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    // Relative to the installer's working directory, the HOME.
    let old = machine.home.path().join("old");
    std::fs::create_dir(&old).expect("create the old install directory");
    std::fs::write(old.join("hip"), "the 0.14.0 binary").expect("write the old hip");

    let out = machine
        .installer(no_server(), &["1.0.0"])
        .env("HIP_INSTALL_DIR", "old")
        .output()
        .expect("run the installer");

    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("HIP_INSTALL_DIR must be an absolute path, not old"),
        "{stderr}"
    );
    assert_eq!(entries(&old), ["hip"]);
}

/// With both names set, `HIPC_INSTALL_DIR` wins, and the note says so.
#[test]
fn a_new_install_dir_wins_over_the_old_one() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    let target = "x86_64-unknown-linux-musl";
    let (tarball, sum) = machine.tarball();
    let base = serve(release("v1.0.0", target, &tarball, &sums(&sum, target)));
    let old_dir = machine.home.path().join("old bin");
    let new_dir = machine.home.path().join("new bin");

    let out = machine
        .installer(&base, &["1.0.0"])
        .env("HIP_INSTALL_DIR", &old_dir)
        .env("HIPC_INSTALL_DIR", &new_dir)
        .output()
        .expect("run the installer");

    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("note: HIP_INSTALL_DIR is now HIPC_INSTALL_DIR; using HIPC_INSTALL_DIR\n"),
        "{stderr}"
    );
    assert_eq!(entries(&new_dir), ["hipc"]);
    assert!(!old_dir.exists());
}

/// The note on a `hip` that may be this tool under its old name, at `path`.
fn old_hip_note(path: &Path) -> String {
    format!(
        "note: {} may be this tool under its old name (hip, up to 0.14.0); if so, remove it\n",
        path.display()
    )
}

/// `hip` was this command's name up to 0.14.0, and an upgrade leaves it behind: one in the
/// install directory and another on PATH each get a note, and a run with neither gets none.
#[test]
fn an_old_hip_binary_gets_a_note_to_remove_it() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    let target = "x86_64-unknown-linux-musl";
    let (tarball, sum) = machine.tarball();
    let base = serve(release("v1.0.0", target, &tarball, &sums(&sum, target)));

    let out = machine.install(&base, &["1.0.0"]);
    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(!stderr.contains("under its old name"), "{stderr}");

    let dir = machine.install_dir();
    std::fs::write(dir.join("hip"), "#!/bin/sh\necho hip 0.14.0\n").expect("write an old hip");
    // Executable, so `command -v` finds it when the install directory is on PATH below.
    std::fs::set_permissions(dir.join("hip"), std::fs::Permissions::from_mode(0o755))
        .expect("make the old hip executable");
    machine.fake("hip", "echo hip 0.14.0");

    let out = machine.install(&base, &["1.0.0"]);
    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    for path in [dir.join("hip"), machine.bin.join("hip")] {
        let note = old_hip_note(&path);
        assert!(stderr.contains(&note), "{note}: {stderr}");
    }

    // The hip on PATH is the one in the install directory: one note, not two.
    std::fs::remove_file(machine.bin.join("hip")).expect("remove the fake hip");
    let path = format!("{}:{}", machine.bin.display(), dir.display());
    let out = machine
        .installer(&base, &["1.0.0"])
        .env("PATH", &path)
        .output()
        .expect("run the installer");
    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert_eq!(stderr.matches("under its old name").count(), 1, "{stderr}");
}

/// A `hip` symlink whose target is gone is the same leftover, and gets the note too.
#[test]
fn a_dangling_hip_symlink_gets_the_note_too() {
    let machine = Machine::new("Linux", "x86_64", &["curl", "sha256sum"]);
    let target = "x86_64-unknown-linux-musl";
    let (tarball, sum) = machine.tarball();
    let base = serve(release("v1.0.0", target, &tarball, &sums(&sum, target)));
    let dir = machine.install_dir();
    std::fs::create_dir_all(&dir).expect("create the install directory");
    symlink(machine.home.path().join("gone"), dir.join("hip")).expect("link a dangling hip");

    let out = machine.install(&base, &["1.0.0"]);

    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let note = old_hip_note(&dir.join("hip"));
    assert!(stderr.contains(&note), "{note}: {stderr}");
}
