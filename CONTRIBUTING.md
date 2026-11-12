# Contributing to hipc

Thanks for helping. Bugs and ideas go in an issue first; a security problem goes in a
private report, as [SECURITY.md](SECURITY.md) says. A small fix can come straight as a pull
request.

## What to read

- [TENETS.md](TENETS.md) is the contract: output, errors, exit codes, confirmation, secrets.
  A change that breaks it is a breaking change.
- [ARCHITECTURE.md](ARCHITECTURE.md) maps the code and holds the rules it keeps: the layers,
  where routes live, how to add a command or a route, the error and output layers, the test
  layout, and the lint, format, and comment policy.
- `crates/hippius` is the library: HTTP, the config and AWS files, errors, the account,
  project, and billing header rules, and the list shapes. It never prints.
- `crates/hippius-cli` is the `hipc` binary: argv, the output mode, every byte of output, and
  the steps of each command.

## Build

`rust-toolchain.toml` pins the toolchain, so `rustup` installs the right one on first use.

```sh
cargo build --locked
./target/debug/hipc --help
```

## Checks

Every pull request passes these. CI runs fmt, clippy, doc, test, deny, coverage and the script
and workflow linters on every pull request.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test --workspace --locked
cargo deny check
```

The `hipc schema` document, its error table and a set of dry runs are held by `insta`
snapshots (ARCHITECTURE.md, "Tests"). A change that moves any of them fails until the
snapshot is refreshed with `INSTA_UPDATE=always cargo test -p hippius-cli`; read the diff of
the `.snap` files and commit it with the change, so a reviewer sees every changed line.

`cargo deny` comes from `cargo install --locked cargo-deny`. A change to a shell script also
passes `shellcheck` and `shfmt -d -i 2`, and a change to a workflow passes `actionlint` and
`zizmor`. actionlint reads only `.github/workflows`, so the `run:` blocks of the release
action in `.github/actions` are shellchecked by a test of the scripts suite instead, and
`zizmor` audits the action. CI runs all four as the lint job, with shellcheck 0.11.0, shfmt
3.14.1, actionlint 1.7.12 and zizmor 1.31.1, so a local run with those versions matches it:

```sh
shellcheck -s sh scripts/install.sh && shellcheck scripts/*.sh
shfmt -d -i 2 scripts
actionlint
zizmor --offline .github
```

Coverage is a gate: every line under `src/` and `build.rs` runs under the test suite.

```sh
cargo llvm-cov --workspace --locked --include-build-script --fail-uncovered-lines 0 \
  --fail-uncovered-functions 20 --show-missing-lines
```

On Linux, CI's coverage run is the test run: every test runs there once, under umask 0002
(Ubuntu's default for a user) so a test that relies on a stricter umask fails. macOS runs
the suite under its default 0022.

The gate is the exit status and the `Uncovered Lines` list. The summary table's `Missed
Lines` is not what it checks: that column scores each function by its best single copy (a
test binary or a generic instantiation) and counts every line of a closure that never runs,
while the gate counts a line as run when any code on it runs in any copy. So a closure that
never runs, sharing its line with code that does, passes the line gate; the
`--fail-uncovered-functions` ratchet catches a new one. Its number is the count on `main`
today and only goes down: lower it when a change removes an uncovered function, and never
raise it.

It is measured on Linux; on macOS the one Linux-only path (`/dev/full`) shows as missed.
Every behavior gets a test, its edge cases and errors included. A line no test can reach is
a design problem: change the design, not the gate.

## Tests never call the live API

No test reaches `api.hippius.com`. End-to-end tests run the `hipc` binary against a local
listener on `127.0.0.1` with a fake token. A new command gets such a test, and a row in
`scripts/release-check.sh`, which maintainers run against a release with a real token. That
script stays read-only against the live API: only GETs reach it, and every write runs against
a dead loopback origin.

## Manual end-to-end test

`scripts/manual_test.sh` walks a person with a real token through the user journeys in
[docs/user-journeys.md](docs/user-journeys.md), against the live API: first run and login,
account and billing, Drive, S3, Hub, compute, CDN, the team, support, agent use, and token
hygiene. It installs the latest release, or tests `--hipc <path>`. Each scenario runs in a
fresh temporary home that logs in and out. Every command is printed with its output, its
exit code, and PASS or FAIL, and the transcript also goes to `manual_test.log`.

```sh
HIPPIUS_TOKEN=<token> scripts/manual_test.sh                # reads only; writes are dry runs
HIPPIUS_TOKEN=<token> scripts/manual_test.sh --tier write   # also creates and deletes free resources
scripts/manual_test.sh --list                               # the scenarios and their commands
```

The token reaches `hipc auth login` on stdin, never argv, and is masked in the output. The
account is left as it was found:

- The write tier creates only a bucket, an S3 sub token, an SSH key, and a Hub key, and
  deletes each one in the same scenario.
- A cleanup ledger deletes anything left over after a failure or Ctrl-C.
- A snapshot of the account taken before the run must match one taken after it.
- Nothing that spends credits or changes a subscription runs except as `--dry-run`.

The test suite runs the script with `--offline`, against a dead origin, so a change to a
scenario updates the script and its block in the catalogue together.

## Docs

Documentation is part of the command (tenet 16). The layers, each short and pointing to the
next, are listed in [docs/README.md](docs/README.md): the README, `docs/getting-started.md`,
`docs/agents.md` and its topic pages under `docs/agents/`, and `docs/user-journeys.md`. The
binary is the reference: what one command takes and does is said by its `--help` and by
`hipc schema`, never by a page.

- Help text is the contract's prose. The short help is one sentence in sentence case with no
  final period and the flag the command needs at the end (`; needs --yes`); the long help says
  what the command sends, reads first, and refuses; every page ends with examples that parse.
  No backticks and no `<https://...>` link syntax in help: it is shown unchanged in the
  terminal, in `hipc schema`, and on the web. The unit tests in `crates/hippius-cli/src/cli.rs`
  hold the shape (one sentence, sentence case, no final period, no backticks, examples that
  parse); review holds the content (what it sends, reads first, and refuses, and the flag it
  names).
- A page teaches how to use `hipc`, not what each command takes: the grammar, how to find a
  command, the output and error contract, the safety rules, and a few examples chosen to show a
  point. It does not list a command's flags or every command of a group; it says how to ask
  the binary. So adding or removing a command changes no page unless it changes how an agent
  starts, or a rule that holds across commands.
- `docs/agents.md` is what an agent reads first, and it stays under 10 KB; each page under
  `docs/agents/` stays under 6 KB and is opened only when a task needs it. A test holds both
  sizes. Before adding a paragraph, check that `--help` or `hipc schema` cannot say it.

- A change to a command's surface (a command, a flag, an effect, a confirmation, a guard, an
  error code) updates, in the same pull request: `CHANGELOG.md`; the pages that show the
  command, if any; and, when an
  agent's first steps change, the `llms.txt` and the CLI section of docs.hippius.com (once
  it is published), whose source is the public `hippius-doc` repository. The contract tests
  run every `hipc` command the README, `docs/getting-started.md`, and the agent pages show,
  so a renamed command or flag fails until the pages follow.
- Prose follows tenet 15: plain sentences, literals in backticks, no exclamation marks, no
  "please", and nothing private: no internal repository, file, or person's name.

## Style

- Errors follow the phrasing of tenet 15: lowercase, no final period, every literal in
  backticks, and a hint that is one imperative naming the fix.
- No new dependency without a reason in the pull request. Prefer the standard library.
- No `unsafe` code; the crates forbid it.
- A code comment says why, not what: an invariant, a source, a rejected alternative.
- The lint, format, and comment rules, and where they depart from the house Rust style, are in
  [ARCHITECTURE.md](ARCHITECTURE.md).

## Commits and pull requests

- One logical change per commit. The subject is imperative and at most 72 characters, such
  as `Refuse a firewall rule key the API does not list`; the body says why.
- A user-visible change adds a line under `## [Unreleased]` in [CHANGELOG.md](CHANGELOG.md).
- Open the pull request as a draft until the checks above pass, then mark it ready for
  review. The pull request template asks what changed, why, how you tested it, and walks the
  checklist above: tests, the CHANGELOG line, help text, the regenerated reference, the docs
  pages, and no private references.
- By submitting a contribution, you agree it is licensed as the README's License section
  says.
