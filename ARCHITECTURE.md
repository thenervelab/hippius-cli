# Architecture

How the code of `hipc` is laid out and the rules it keeps, for the people who change it.
[TENETS.md](TENETS.md) is the contract with callers and wins every disagreement;
[CONTRIBUTING.md](CONTRIBUTING.md) lists the checks a pull request passes. Nothing here is
visible to a caller, so breaking a rule below is not a breaking change, but review refuses it
as it refuses a broken tenet.

Parts marked **planned** describe the design the code is moving to. Until they land, the
code is the "today" part, and a change follows that.

## Codemap

Two crates. A layer calls down, never up.

```
crates/hippius-cli   the `hipc` binary: argv, the output mode, every byte of output
  main.rs            entry point; calls `run::run` and exits with its code
  run.rs             parses argv, builds the context, dispatches one command, hands the result to output
  cli.rs, cli/       the clap grammar, one file per group; its doc comments are the help text
  commands.rs, commands/
                     the steps of each command, one module per group; each returns `Printed`
  context.rs         what a command runs with: global flags, the terminal, and the steps of
                     every request (dry run, confirmation, then the network; tenet 5)
  spec.rs            what each command does to the server, for `hipc schema` (tenet 7)
  output.rs          every byte written to stdout and stderr (tenets 3, 4 and 15)
  hidden.rs          a secret typed at the terminal, read without echo
crates/hippius       the library: never prints, never reads argv
  client.rs          the HTTP client: origin, headers, limits, redirects, the answer
  account.rs         which account a request acts in, and the route policy tables
  project.rs         which project and billing account a request acts in
  config.rs          the config file and the settings of one run (tenet 8)
  json.rs            the list shapes of the API and the list envelope
  private_file.rs    files only the caller may read: config, AWS files, secret files
  private_object.rs  a private file that holds one JSON object of secrets
  aws.rs             the AWS CLI profile that `hipc s3 configure` writes
  kubernetes.rs      the cluster key: signatures and the sealed kubeconfig
  wallet.rs          BIP-39 phrases and the keys they derive
  app_secrets.rs, dns.rs
                     the secret input files of App Engine and the CDN DNS setup
  leaf modules, which import no module above them:
  error.rs           `Error` and `Code`: the JSON error object and the exit codes (tenet 4)
  redact.rs          the spellings of a secret, and their redaction from text and JSON (tenet 9)
  secret.rs          JSON values that hold secrets, cleared when dropped
  method.rs          the HTTP method, and whether a request may change state
  selector.rs        the account, project and billing account a run selects
  url.rs             API origins, and URLs with their query values withheld
  time.rs            the HTTP `Date` and RFC 3339 forms, and the time now by the API's clock
  encode.rs          hex and percent-encoding
  ids.rs             random UUIDs: request ids and idempotency keys
  phrase.rs          the recurring pieces of a message, built one way (tenet 15)
  regular.rs, source.rs, digest.rs
                     file opening, where a value came from, SHA-256
```

The layering:

- `main` calls `run`; `run` calls `cli` (parse), `commands` (steps) and `output` (print).
- `commands` call `context` and the library. They return what they print (`Printed`) or an
  `Error`, and never write output themselves; only `output` writes, and `context` reaches it
  for prompts, `--debug` lines and notes.
- `cli` is data: `commands` and `spec` read its types; it calls nothing above it.
- The library knows no command, no clap type and no output mode. `hippius-cli` reaches it
  only through the `pub use` list in `crates/hippius/src/lib.rs`.
- Inside the library, `client` consults `account` and `project` for the header rules, and
  everything returns `error::Error`. The modules import each other without a cycle: a leaf
  module imports only other leaves, `account` imports leaves, `project` imports `account`,
  `config` imports no route module, and `client` sits on top. Check it with
  `rg '^use crate::' crates/hippius/src` before adding an import that points up.

## Routes

### Today

A route is spelled in several places, in two dialects:

- A command builds its request from a path without `/api` (`Request::get("/compute/vms/")`).
  `Client` prepends `/api` to any target that lacks it; that is the one version seam.
- The library's policy tables match paths with `/api`: `SCOPED_PREFIXES`,
  `PERSONAL_PREFIXES`, `PERSONAL_PATHS`, `SECRET_ROUTES`, `PROVISIONING_ROUTES`,
  `PERMANENT_ROUTES` and `ROUTE_RULES` in `account.rs`, and the project and billing tables
  in `project.rs`. They decide the account and project headers (tenet 12), what reveals a
  secret (tenet 9), what creates on a read, and what cannot be undone.
- `commands/api.rs` keeps `CONSEQUENCES`, the routes `hipc api` confirms with their effect.
- `spec.rs` and the help prose name routes in text.
- The end-to-end tests, `scripts/release-check.sh` and `scripts/manual_test.sh` hold their
  own literal paths.

So a new route, or a route that moves, is a search across both crates, the tests and the
scripts. A missed policy row changes header or guard behaviour silently.

### Planned: `hippius::routes`

One module tree in the library, `crates/hippius/src/routes/`, one file per service, holds
every endpoint `hipc` calls as a constant:

- its method, and its path with the API version in it (`/api/compute/vms/{vm_id}/`);
- its header class (scoped, personal, unsupported; tenet 12) and how it takes the project and
  billing headers;
- whether its answer reveals a secret, whether a read creates something, whether it cannot be
  undone;
- a request builder that fills each placeholder, percent-encoded as one path segment.

Rules that come with it:

1. Only `routes` spells a path. A command builds its request from the constant.
2. An agreement test holds every row against the policy tables while both exist; then the
   client consults the row first and falls back to the tables only for a path with no row,
   so an unknown route still fails closed.
3. `hipc api` matches against the same table, so a guard added for a curated command guards
   the raw route too. A route with no row is treated as possibly secret on a read and
   unknown on a write.
4. `hipc schema` exports each command's method and route templates, and the release scripts
   read them from there instead of keeping their own copy.
5. A request body is a `Serialize` struct beside its route. An answer stays a
   `serde_json::Value` and is printed as it came (tenets 3 and 15). A field `hipc` decides on
   is read through a small `Deserialize` view beside the route, never printed, never through
   `flatten` or `untagged`.

## How to add a command (today)

1. The clap type in `cli/<group>.rs`: short help one sentence, long help says what it sends,
   reads first and refuses, then examples that parse (CONTRIBUTING, "Docs").
2. The steps in `commands/<group>.rs`, returning `Printed`; dispatch from the group's `run`
   (or `run.rs` for `auth` and `account`).
3. Its row in `spec.rs`. A test walks the parser and fails when a command has no row.
4. Its route in the library's policy tables (account, project, secret, provisioning,
   permanent) when the route is new to `hipc`; **planned**: one row in `routes`.
5. An end-to-end test in `crates/hippius-cli/tests/e2e/` against the local server, and a row
   in `scripts/release-check.sh` (read-only against the live API).
6. A regenerated `docs/commands.md`, a `CHANGELOG.md` line, and the docs pages that show it.

## How to add a route or an API version (planned)

Until `routes` lands, a new route is a path in the command plus a row in each policy table
it belongs to. Once it lands:

- A new route is one constant, in its service's file, with its class and flags. The
  agreement test covers it.
- A new API version of an endpoint, say `GET /api/v2/compute/vms/{vm_id}/`, is a new
  constant beside the old one with the same class and flags. The commands that move to it
  change their constant. If `hipc` decides on a renamed field, its view struct changes in one
  place (`#[serde(alias = "status")] state`).
- A printed document that changes shape needs a new `schema_version` (tenet 15), and is
  listed in `CHANGELOG.md`.
- The old constant goes when no command sends it. `hipc api` reaches both versions
  throughout. The version is never a client setting, and `hipc` never negotiates it at run
  time: a release sends the version of each route it was tested against.
- A `Deprecation` or `Sunset` header on an answer is surfaced as a warning, and a `410` on a
  curated route gets a hint to upgrade `hipc`; tenet 15 gains the line when this lands.

## Error model

There is one error type, `hippius::Error`, in both crates; neither uses `anyhow` or `eyre`.
An `Error` carries a `Code` from the closed table of tenet 4 (which fixes its name and exit
code), a message, an optional hint, optional details, and what was already done
(`SideEffects`). `Code::ALL` lists every code for `hipc schema`. A new code is a contract
change, not a refactor. Messages follow tenet 15: lowercase, no final period, literals in
backticks, a hint that is one imperative; `phrase.rs` builds the pieces that recur. A
message never quotes a secret or server text that could hold one, which is why errors drop
their causes on purpose instead of chaining them.

## Output layer

`output.rs` writes every byte, with `write_all` (a closed pipe must not panic). A command
returns `Printed`: a `Document`, a `List` (the envelope of tenet 6), `Raw` bytes, or a `Body`
from `hipc api`. `output` renders it in the mode `run` resolved (JSON or text). In JSON mode
a failure is exactly one error object on stderr; in text mode a short paragraph. Text for a
person is escaped so no server value can move the cursor or disguise a name. Notes, prompts
and `--debug` lines go to stderr through `context`.

## Tests

- **Library unit tests**: `#[cfg(test)] mod tests` at the bottom of each file, or a
  `tests.rs` beside it (`kubernetes/tests.rs`).
- **CLI unit tests**: in `cli.rs` (help shape, examples parse), `spec.rs` (every command has a
  row), `output.rs`, `run.rs`.
- **End to end**: `crates/hippius-cli/tests/e2e/`, one test binary (`main.rs` declares a
  module per area). Each test runs the built `hipc` against a local server on `127.0.0.1`
  (`common.rs`), with a fake token, and checks the requests it sent. No test reaches the live
  API.
- **Scripts**: `crates/hippius-cli/tests/scripts/` runs the shell scripts in `scripts/` and
  the release workflow's guards against local files and a local server.
- **Coverage** is a gate: 100% of lines under `src/` and `build.rs`, and a ratchet on
  uncovered functions that only goes down. `cargo llvm-cov`'s summary table has a "Missed
  Lines" column that counts each function by its best single copy (a test binary or a generic
  instantiation) and every line of a closure that never runs. `--fail-uncovered-lines`
  checks something else: a line counts as run when any code on it runs in any copy. So read
  the gate's exit status and its `Uncovered Lines` list, not the column (CONTRIBUTING,
  "Checks"). A closure is a function of its own, so a fallback closure that never runs
  (`unwrap_or_else(|_| dir.clone())` where the error cannot be produced in a test) raises
  the uncovered-function count; borrow the fallback (`.ok()`, then `unwrap_or(&dir)`) or pass
  a function item (`map_or(Ok(()), create_private_dirs)`).
- **Planned**: snapshots (`insta`) only for documents `hipc` wholly owns: `hipc schema`, the
  error table, and dry-run documents. Anything else is checked by the fields that matter.

## Lint, format and comment policy

`hipc` follows the house Rust style `rust-style` (opted in with `baseline = true` under
`[workspace.metadata.house-check]` in `Cargo.toml`, rule REPO-1). Where it and TENETS
disagree, TENETS wins. Its rule IDs are named in reviews and commit messages, never in code
comments.

- **Lints** (`Cargo.toml`, `[workspace.lints]`): rustc's 2018 idioms, `missing_docs`,
  `unreachable_pub`; clippy `pedantic` with four reasoned relaxations; and `restriction` and
  `nursery` lints chosen one by one, never as groups. They warn, and CI denies every warning.
  A lint is enabled only when it has no hit left, so it is a ratchet.
- **Suppression**: `#[expect(lint, reason = "...")]` on the smallest item, never `#[allow]`
  (which never reports going stale) and never for a file or a crate (LNT-1). The one
  `#[allow]` is for rustdoc lints, which rustc and clippy do not run, so an `expect` of them
  is never fulfilled (`main.rs`).
- **`clippy.toml`**: tests may unwrap, expect and print; at most 5 parameters and 100 lines
  per function; product names for `doc_markdown`; `serde_json::Map` removals that reorder the
  server's fields are banned.
- **`deny.toml`**: advisories, licences, sources, no OpenSSL or native-tls (TLS is rustls),
  and a duplicate crate version fails unless it is listed with its reason.
- **Format**: `cargo fmt` with `rustfmt.toml`, which holds stable options only. Imports are
  grouped by hand: `std`, then other crates, then `crate`, one blank line between.
- **Names and bindings**: whole words; a `let` where the value is first used; `let ... else`
  for an early return; an ignored value bound to a name that says why, never `_`; an enum,
  not a `bool`, where the call site could not tell what the `bool` means.
- **Comments** say why: an invariant, a source, a rejected alternative. One that rests on the
  server names where it was read (the API schema, a console behaviour); one that rests on the
  contract cites the tenet instead of repeating it. Prefer a name, a type, an `expect` that
  states its invariant, or a test to a comment. A doc comment opens with one sentence, and a
  public library function that can fail says how under `# Errors`.

### Where `hipc` departs from the house templates

- **`rustfmt.toml` exists.** The house style keeps none. `hipc` keeps a stable-only one
  because `rustfmt` run outside cargo (an editor) formats as the 2015 edition without it. Its
  editions must match `Cargo.toml`'s.
- **Duplicate versions fail** (`multiple-versions = "deny"`, the template warns): the
  duplicates `hipc` carries are few and listed with their reasons, so a new one is a decision.
- **TLS crates are banned** by name in `deny.toml`; the template bans nothing.
- **Unused template entries are dropped**: `module_name_repetitions` (a restriction lint now,
  off unless named) and the template's custom `cfg` names, which the code does not use.
- **No `rust-version`.** Both crates are unpublished; the workspace pins its toolchain in
  `rust-toolchain.toml`, as the house rule asks of an application (REPO-11).
- **No `anyhow` in the binary.** The house allows it in a CLI; `hipc`'s error object is the
  contract (tenet 4), so both crates use `hippius::Error`.
- **`unreachable!` is linted** (`clippy::unreachable`): an `expect` stating the invariant, or a
  `let ... else` that returns, takes its place, as for `unwrap`.
