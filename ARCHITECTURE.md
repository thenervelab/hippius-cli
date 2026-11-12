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
  cli.rs, cli/       the clap grammar, one file per group; its doc comments are the help text.
                     cli/ids.rs holds the id types and the argument each command acts on;
                     cli/values.rs the one rule of names, ids, labels, and catalog codes
  units.rs           exact decimal amounts: the one grammar of an amount, credits, dollars,
                     and planck
  commands.rs, commands/
                     the steps of each command, one module per group; each returns `Printed`
  context.rs         what a command runs with: global flags, the terminal, and the steps of
                     every request (dry run, confirmation, then the network; tenet 5)
  spec.rs, spec/     what each command does to the server, for `hipc schema` (tenet 7): one
                     function of rows per command group (`spec/rows.rs`), and its routes
  output.rs          every byte written to stdout and stderr (tenets 3, 4 and 15)
  hidden.rs          a secret typed at the terminal, read without echo
crates/hippius       the library: never prints, never reads argv
  client.rs          the HTTP client: origin, headers, limits, redirects, the answer
  routes.rs, routes/ every endpoint `hipc` calls, one constant per method and path, one file
                     per service, with its policy: the row decides
  account.rs         which account a request acts in, and the policy of routes with no row
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
  redact.rs          the spellings of a secret, the names of secret fields, and their
                     redaction from text and JSON (tenet 9)
  secret.rs          JSON values that hold secrets, cleared when dropped
  method.rs          the HTTP method, and whether a request may change state
  selector.rs        the account, project and billing account a run selects
  url.rs             API origins, and URLs with their query values withheld
  time.rs            the HTTP `Date` and RFC 3339 forms, and the time now by the API's clock
  encode.rs          hex and percent-encoding
  ids.rs             random UUIDs: request ids and idempotency keys
  phrase.rs          the recurring pieces of a message, built one way (tenet 15)
  view.rs            how a typed view reads an answer's fields (see "Bodies and views")
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
  module imports only other leaves, `routes` imports leaves, `account` imports leaves and
  `routes`, `project` imports `account`,
  `config` imports no route module, and `client` sits on top. Check it with
  `rg '^use crate::' crates/hippius/src` before adding an import that points up.

## Routes

### The table: `hippius::routes`

`crates/hippius/src/routes/` holds every endpoint `hipc` calls, one file per service
(`account`, `billing`, `cdn`, `compute`, `databases`, `drive`, `hub`, `kubernetes`,
`network`, `runners`, `s3`, `support`). Each is a constant in a `routes!` table:

```rust
/// Removes a member from the team.
MEMBER_DELETE: 1 = Route::delete("/api/accounts/members/{member_id}/")
    .scoped()
    .guard(Guard::NeedsWhenSet(&[RouteFlag::RemoveOthers], "grants"));
```

- The path is the whole path under the origin, the API base and any version segment
  included (`routes::API_BASE` is `/api`; a constant that does not start with it does not
  compile). A placeholder is `{name}`, one segment, or `{name..}`, one or more segments (a
  Hub repository); its name is the argument of the command that fills it.
- The number is the count of placeholders, part of the type (`Route<1>`), so a request built
  from the constant fills each one, checked when it compiles.
- The methods after it say the route's policy: its account class (`scoped`, `personal`, or
  `own_account`, which refuses a membership; the `routes!` table refuses a row without one),
  how it takes a project and a billing account (`project`, `billing`, `names_one`,
  `beside_billing`), whether its answer `reveals` a secret, whether a read `provisions`,
  whether it is `permanent`, whether its 503 means it was `undone_on_503`, how the server
  pages its list (`paged(Paging::Numbered)`, `Totalled`, `UntilShort`, `Cursor`,
  `Continuation`: the query
  that asks for a page and the answer's shape), and the `guard` `hipc api` holds it back with.
- Names: a list is the plural (`MEMBERS`), one item the singular (`MEMBER`), and a write the
  noun and its verb (`MEMBER_UPDATE`, `MEMBER_DELETE`, `INVITE_CREATE`).

A command builds its request from the constant: `Request::to(&routes::account::MEMBER_DELETE,
[member_id])`. `Route::path` percent-encodes each value as one path segment (a `{name..}`
value keeps its `/`), and the client refuses a path with a percent-encoding, so a value that
needs one is refused before anything is sent rather than reaching another route. The
request keeps its target as `hipc api` takes it, without the base, so a hint that names the
route (`run hipc api GET /accounts/members/`) reads as a caller types it; the client adds
`API_BASE` back. `API_BASE` is also where `hipc api` puts a path given without it: the one
place the client knows the base.

`hipc schema` lists each command's `routes` (`spec/routes.rs` maps each command to its
constants), and `scripts/release-check.sh` and `scripts/manual_test.sh` read a route from
there (`schema_route` in `scripts/lib.sh`) rather than keep their own copy.

### The row decides; the tables serve routes with no row

The client looks up the row of every request first (`routes::row`, by the path alone, as the
server's router picks a route: the template with the most literal segments among those that
match, then the method; a `HEAD` reaches the `GET` row) and takes its
policy: the account class, project and billing handling, whether the answer reveals a
secret, whether a read provisions, whether the route is permanent, whether a 503 means it was
undone, and the guard `hipc api` applies. A guard that refuses carries why and the one path to
the route's effect; the message names the method the request sent, so a write method that
borrows the guard of another write method of its route names its own. A write method with no
row borrows only from the rows of the template its path reaches (`routes::route_at`,
`Guard::lent_to`); a `Guard::Distinct` row says its method does something else and lends
nothing. The path is tried in each form the tables match (slashless, with a format suffix,
in another letter case). The secret, provisioning, permanent-delete, and guard checks also
read it under every other API version (`account::in_every_version`), so a refused or secret
route stays so on a version the API adds before a row names it; the row lookup itself
reads the path only as written, so `hipc api` still treats such a path as a route it does
not know.

A placeholder's name says what fills it, and for a few names, the shape that fills it
(`fits` in `routes.rs`, the one place a name decides a shape): `database_id` and
`invoice_id` are digits, as the server's router takes them; `project_id` and
`billing_account_id` are ids of their kind; a Hub `model` is two segments. A path with
another value there is another route: `/api/databases/metrics/` is not one database's route,
and `/api/accounts/projects/default/` not one project's, so the tables decide them. A new
template that names its placeholder anything else takes any value.

The tables in `account.rs` and `project.rs` (`SCOPED_PREFIXES`, `PERSONAL_PREFIXES`,
`PERSONAL_PATHS`, the project and billing tables, and the secret and guard tables of the
routes only `hipc api` reaches) decide only a path no row matches, and fail closed for one
they do not list either (tenet 12). `hipc api` treats a route with no row as one `hipc`
does not know: a `GET` needs `--reveal`, and a write warns (TENETS 7 and 9). These tests keep
the two apart:

- `routes::tests::the_header_tables_never_contradict_a_row`: where the header tables list a
  path a row decides, its own or a literal path its placeholder takes, they say what the row
  says, since they still decide the other methods of that path. A path they do not list is
  the row's alone. It lists every disagreement at once.
- `account::tests::no_table_entry_is_a_route_a_row_decides`: the secret and guard tables hold
  only routes no row decides, so nothing is kept twice.
- `routes::tests::every_row_is_reached_by_its_own_path`, and
  `the_most_literal_row_wins_and_a_head_reaches_the_get`: the lookup itself.
- `spec::tests::every_listed_route_is_a_row_that_pages_as_its_command_does`: every route
  `hipc schema` lists is a row, and a command takes the page flags exactly when one of its
  routes is `paged`; `spec::tests::every_row_is_a_route_some_command_lists`: the converse,
  so "a route no command lists" in `hipc schema` is exactly a route with no row.
  `pagination::every_route_here_pages_as_its_row_says` holds the end-to-end paging tables
  to the rows too.
- Every request an end-to-end run sends is checked against the routes `hipc schema` lists for
  the command (`tests/e2e/common/routes.rs`), so a command that starts sending to another
  route fails until it is listed.

### Still spelled outside the table

Every command builds its requests from `hippius::routes` with `Request::to`, and the
end-to-end tests serve the same constants (`common::route_of`, and `common::route_each_of` for
a route that answers several times). Two kinds of request are still built from a written
target, with `Request::new`, and `commands::tests::only_hipc_api_builds_a_request_from_a_written_target`
fails on any other:

- `hipc api` (`commands/api.rs`), which sends the route it is given.
- A dry-run placeholder that `Route::path` would percent-encode, and the client then
  refuse: `hipc hub subscription subscribe --replace` sends `SUBSCRIPTION_CANCEL`'s template
  with `{subscription_id}` as it is. (`hipc compute vms create --open-port` builds its dry
  run from `Request::to` with a plain stand-in id, and writes `<the vm_id the launch
  returns>` into the URL afterwards.)

Help prose that names a route, and the end-to-end tests' own literal paths, stay as
written: a test that spells the path checks the constant independently. So do the tests
that serve a route with a query string, which `route_of` does not cover.

## Bodies and views

What `hipc` sends is typed; what the server answers is printed as it came.

- **A body** is a `#[derive(Serialize)]` struct in the route's file, `routes/<service>.rs`,
  below the `routes!` table, named after its route: `MEMBER_UPDATE` takes
  `MemberUpdateBody`. Its fields are in the order the wire carries them, and a key the
  request leaves out is `#[serde(skip_serializing_if = "Option::is_none")]`; a field that is
  sent as `null` is a plain `Option`. The end-to-end tests assert the bytes sent, so a field
  moved or renamed fails them. One shape several routes take is one struct whose doc names
  each (`NameBody`); a body that holds a secret writes its own `Debug`, which leaves it out.
  `Request::json` takes the struct. The empty body `{}` some POSTs take is
  `routes::EmptyBody`.
- **A view** is a small `#[derive(Deserialize)]` struct in the same file, with the fields a
  command decides on and nothing else, read from the answer's `Value` and never printed:
  `let offer: InviteOffer = hippius::view_of(&answer);`. A view that replaces
  `answer.get("field").and_then(Value::as_str)` reads each field with
  `#[serde(default, deserialize_with = "field")]` and is read with `view_of`, so a field that
  is missing, `null`, or of another type is `None`, as the `Value` read was, and an answer that
  is not an object reads as all `None`. A view inside a view reads with `nested`, and a list of
  views with `each`, so an item that is not an object is a view of `None`s, as
  `answer.pointer(..)` and a loop of `get`s saw them. A decision that tells a missing field from
  `null`, or from one of another type, reads a `hippius::Slot` (the VM launch's
  `backup_error`, an operation's `node`). A view a command needs whole (the hAlpha quote) takes
  plain fields and is read with `deserialize`, whose error the command turns into its own
  `internal`.
- **Never** `#[serde(flatten)]`, an untagged enum, or an internally tagged one in a view:
  `serde_json`'s `arbitrary_precision` buffers a number through them as a private map and the
  read fails ([serde-rs/json#559](https://github.com/serde-rs/json/issues/559)). Never
  `deny_unknown_fields`: a field the server adds must change nothing (tenet 15).
- **Numbers** a decision depends on stay exact: an integer is `u64` (which takes only an
  integer that fits, as `as_u64` did), an 18-decimal amount is the digits (`planck` in
  `routes/billing.rs`), and a number kept as sent is a `serde_json::Number`.
- **A server vocabulary** (a purchase status, an auto-reload state, a VM status, a Kubernetes
  operation's state and kind) is an enum in the route's file with an `Other(String)` variant,
  so a value the server adds is not an error, and an `as_str` that gives the server's spelling
  back for a message. The `vocabulary!` macro in `routes.rs` declares one. A server catalog (an S3
  plan, a backup interval, a database tier, size, or engine version, a runner size, a Hub key
  role) is a validated string, not a closed enum, so a code the server adds needs no release
  (owner decision D5): `cli::values::catalog` checks its shape and the help lists the codes known
  today. Where the server lists the catalog, the write is sent through
  `commands::send_from_catalog`, which reads it after the question and refuses a code it does
  not list, as Hub's `--plan` is resolved.
- **A dry run** is a `context::DryRun`, which only `Context::dry_run_of(client, &[&read,
  &write])` builds: every request a real run sends, in order, then the fields some commands
  add through its methods (`reads_first`, `may_cancel_first`). A command that changes only a
  local file uses `commands::local_dry_run`, whose fields are a `serde_json::Map`, so none
  can be dropped. No command writes a field of a dry run by name.

## Confirmations

A command's schema row (`spec/rows.rs`) says whether it asks before a write (tenet 5), and
the row gates the write: `run.rs` looks the row up once and hands its `confirmation` to the
`Context`. `Context::confirm_write` returns a `Confirmed`, and `Context::send_confirmed`, which
takes one, is the only way to send a write of a command whose row asks; `Context::send` takes
reads, and writes of a command whose row never asks, and panics on any other write, so a
command that forgets its question fails in the tests rather than sending. A row that asks only
when a condition holds says it did not ask with `Context::unasked`. `Context::call` and
`call_confirmed` (which keeps the `Confirmed` for the later writes one question covered) do the
whole sequence for one request.

A write that reads first, to name its target in the question or to decide whether to ask, goes
through `commands::read_then_write`: a `WritePlan` lists the reads and the write a dry run
shows, and a `decide` closure, run after the write's checks, sends the reads and returns the
write with its question (`Decided::asked`, `asked_as`, or `unasked`), so tenet 5's order holds
by construction. A command whose steps that does not fit (one that reads again after the
question, one question for several writes, a secret put in a body only once confirmed) calls
`Context::check_write` with its `WritePlan` itself, and says why beside the call.

## Lists

`commands::list::walk` is the one page loop: it sends each page, counts it against the caps of
tenet 3 (`Pages`), and admits each next page, so a loop or a list past a cap is refused the same
way everywhere. `list::run` reads a list the way its route's row says the server pages it
(`Request::paging`), into one `hippius::Page` whose `Next` is a link or a token
(`list::read_page`, which holds the readers of the two styles the library does not:
`Totalled`, by `total`, `UntilShort`, while a page is full, and `Continuation`, held to the
`--page-token` rules); `hipc s3 tokens list`, `hipc hub usage --repositories`, and `hipc api
--all` walk their pages through the same loop. `hipc api
--all` follows `next` links only, so a page of a route its row says pages by number or token
that names a next page is `internal` rather than read as the last.

## How `hipc api` says its rules

`hipc api`'s `refused_when`, its guards' `when` and its `confirmation_when` in `hipc schema`,
the help of `--change-isolation`, `--public` and `--remove-others`, and the refusals in its long
help are written by `cli/api/rules.rs` from the route rows and the guard table
(`hippius::api_routes`, `hippius::api_guard`), never by hand. A library test holds `api_guard`
to the rule the client applies, and `e2e/api.rs` `every_refusal_is_named...` checks each
refusal's behaviour and both texts. A route only `hipc api` reaches is a row too, listed under
`["api"]` in `spec/routes.rs`.

## How to add a command (today)

1. The clap type in `cli/<group>.rs`: short help one sentence, long help says what it sends,
   reads first and refuses, then examples that parse (CONTRIBUTING, "Docs"). The thing it acts
   on is the id's `*Target` struct from `cli/ids.rs`, flattened (`Get(VmTarget)`), so its help
   line and parser are written once; a text id is a newtype only its parser builds, which
   `Request::to` takes as it is. A one-line label goes through `cli::values::label`, an amount
   through `units`.
2. The steps in `commands/<group>.rs`, returning `Printed`; dispatch from the group's `run`
   (or `run.rs` for `auth` and `account`).
3. Its row in its group's function in `spec/rows.rs`. A test walks the parser and fails when
   a command has no row, and the `hipc schema` snapshot shows the row.
4. Its routes: a constant in `hippius::routes` for each, built with `Request::to`, and the
   command's arm in `spec/routes.rs` (see "How to add a route"). A write's body is a struct
   beside the route, and a field it decides on is read through a view ("Bodies and views").
5. An end-to-end test in `crates/hippius-cli/tests/e2e/` against the local server, and a row
   in `scripts/release-check.sh` (read-only against the live API), which reads the route
   from `hipc schema`.
6. A `CHANGELOG.md` line, and the docs pages that show it, if any (CONTRIBUTING, "Docs").

## How to add a route or an API version

A new route:

1. Add its constant to the service's file in `crates/hippius/src/routes/`, with its
   policy methods; the row decides. Run `cargo test -p hippius routes:: account::`: where
   the header tables list the path, they must say what the row says, and an entry of the
   secret or guard tables that the row now decides must leave the table.
2. Build the request with `Request::to(&routes::<service>::NAME, [values])`.
3. Add the constant to the command's arm in `crates/hippius-cli/src/spec/routes.rs`. The
   end-to-end check fails on the first request the command sends to a route it does not
   list.

A new API version of an endpoint is the same steps, with the old constant kept. Say the API
ships `GET /api/v2/accounts/members/`, whose answer renames `members` to `results`:

1. Add `MEMBERS_V2: 0 = Route::get("/api/v2/accounts/members/").scoped();` beside
   `MEMBERS` in `routes/account.rs`. Nothing else: `/api/v2/accounts/` is under no prefix of
   `SCOPED_PREFIXES`, so without the row a membership would be refused on it, and `hipc api`
   would treat it as a route it does not know. The row decides, which is why it carries the
   class.
2. Point `members_list` in `commands/account.rs` at `routes::account::MEMBERS_V2`, and
   `["account", "members", ...]` in `spec/routes.rs` at the new constant. Keep the old
   constant listed by moving it to the `["api"]` arm, which lists the routes only `hipc api`
   reaches: a row no command lists fails
   `spec::tests::every_row_is_a_route_some_command_lists`. If `hipc` reads a renamed field
   to decide something (`members` in `member_grants`), change that read in its one place.
3. A printed document that changes shape needs a new `schema_version` (tenet 15) and a
   `CHANGELOG.md` line. To keep the old shape, add the old field beside the new one before
   printing, the one transformation tenet 15 allows.
4. Delete `MEMBERS`, and its line in the `["api"]` arm, when no command sends it. Until
   then `hipc api` knows both versions. After it, `hipc api` treats the old path as a route
   it does not know: a `GET` of it needs `--reveal` and a write warns, which is a
   `CHANGELOG.md` line. A guard or a `reveals` the new row carries still holds for the old
   path, since those checks read a path under every version the table names.

The version is never a client setting, and `hipc` never negotiates it at run time: a release
sends the version of each route it was tested against. A `Deprecation` or `Sunset` header on
an answer is surfaced as a warning (`Client::take_deprecation_warnings`, which
`Context::transmit` moves into the run's warnings after each request, one per route), and a
`410` gets a hint to upgrade `hipc` (tenet 15).

## Error model

There is one error type, `hippius::Error`, in both crates; neither uses `anyhow` or `eyre`.
An `Error` carries a `Code` from the closed table of tenet 4 (which fixes its name and exit
code), a message, an optional hint, optional details, and what was already done
(`SideEffects`). `Code::ALL` lists every code for `hipc schema`. A new code is a contract
change, not a refactor. An exit-1 failure is built with `Error::new`, which takes a `Failure`
(the exit-1 codes alone); every exit-2 code has its own constructor that takes the hint it must
carry (`Error::usage`, `Error::confirmation_required`, ...), so an exit-2 error without one
cannot be written, and no constructor can panic. Messages follow tenet 15: lowercase, no final period, literals in
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

- **Library unit tests**: `#[cfg(test)] mod tests` at the bottom of each file, or, for a large
  file, a `tests.rs` beside it (`client/tests.rs`, `config/tests.rs`, `account/tests.rs`,
  `kubernetes/tests.rs`).
- **CLI unit tests**: in `cli.rs` (help shape, examples parse), `spec.rs` (every command has a
  row), `output.rs`, `run.rs`.
- **End to end**: `crates/hippius-cli/tests/e2e/`, one test binary (`main.rs` declares a
  module per area). Each test runs the built `hipc` against a local server on `127.0.0.1`
  (`common.rs`), with a fake token, and checks the requests it sent. No test reaches the live
  API.
- **Waits**: every command that waits on the server (the VM and App Engine launch waits,
  the hAlpha follow) pauses through one function, `paced` in `commands/poll.rs`. A debug
  build reads `HIPC_TEST_POLL_INTERVAL_MS` there, a whole number of milliseconds above
  zero, and uses it in place of the real 2 or 3 seconds; any other value is ignored. A
  zero pause (the hAlpha follow's first read) stays zero, and a server's `Retry-After` is
  still honoured. The e2e harness (`common::command`) sets it to 50 for every run, so a
  waited launch costs no real time; a test that counts the reads a deadline leaves room
  for removes it (`halpha::top_up_at_the_real_interval`). The code that reads the variable
  is compiled only with debug assertions, so a release build ignores it; the release-build
  action fails a release binary that holds the name, and `tests/scripts` checks that step
  against the debug binary and checks that the release profile keeps debug assertions off.
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
- **Snapshots** (`insta`) hold only documents `hipc` wholly owns: the whole `hipc schema`
  document and its error table (`spec.rs`, kept in `src/snapshots/`), and the dry runs of a
  set of commands that covers each way a dry run is built (`tests/e2e/snapshots.rs`, kept in
  `tests/e2e/snapshots/`). A change to a row, a help line, a route or a dry run shows there
  as one diff; refresh with `INSTA_UPDATE=always cargo test -p hippius-cli`, then read the
  diff before committing it. Anything that carries the server's fields is checked by the
  fields that matter, never by a snapshot.

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
