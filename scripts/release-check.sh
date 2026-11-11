#!/usr/bin/env bash
# Live check of a released hipc binary against https://api.hippius.com, with zero impact.
#
# Usage: HIPPIUS_TOKEN=<token> scripts/release-check.sh <path to hipc> <version> <target>
#
# The token comes only from HIPPIUS_TOKEN; it is never an argument or a file in the
# workspace. Prints one PASS, FAIL or SKIP line per check and never raw output, and exits 1
# when a check fails.
#
# Zero impact, by construction: nothing here may change state on the server, not even a
# change that would be undone:
#
# - A call with the real token, on the real origin, goes through `online`. It runs only a
#   command that `hipc schema` marks read_only (or `hipc api GET`), without `--yes`, and only
#   after the same invocation under `--dry-run` showed nothing but GET requests.
# - Every other call goes through `offline`: a fake token and the origin DEAD_ORIGIN, where
#   nothing listens. Dry-run bodies and refusals (`confirmation_required`, `reveal_required`,
#   `usage`, and the account, project, and billing account scope codes) run there, so a
#   regression that sends instead of refusing reaches no server, and never with the real token.
# - Local writes (`auth login`, `auth logout`, `s3 configure` with a fake secret) go
#   through `local_write`, inside a temporary HOME that the script deletes on exit. So does
#   the invoice download: its request is a GET, and the PDF goes to a new file in the
#   script's temporary directory.
# - A check that depends on account state (a VM, a database, a CDN zone, a membership)
#   prints SKIP when the account has none.
set -euo pipefail

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
refuse_xtrace release-check

# Byte semantics for the 300-byte cut and for grep on raw output.
export LC_ALL=C

if [[ $# -ne 3 ]]; then
  echo "usage: HIPPIUS_TOKEN=<token> $0 <path to hipc> <version> <target>" >&2
  exit 2
fi
HIPC="$1"
VERSION="$2"
TARGET="$3"
TOKEN="${HIPPIUS_TOKEN:-}"
# Trimmed as `hipc` trims it (`Token::new`), so the scans look for the bytes it would print:
# a secret pasted with a trailing newline otherwise never matches.
TOKEN="${TOKEN#"${TOKEN%%[![:space:]]*}"}"
TOKEN="${TOKEN%"${TOKEN##*[![:space:]]}"}"
if [[ -z "$TOKEN" ]]; then
  echo "release-check: set HIPPIUS_TOKEN to a console API token" >&2
  exit 2
fi
if [[ ! -x "$HIPC" ]]; then
  echo "release-check: $HIPC is not an executable file" >&2
  exit 2
fi
command -v jq >/dev/null || {
  echo "release-check: jq is required" >&2
  exit 2
}
# hipc prints the last four characters as `token_preview`; they are redacted too.
PREVIEW="${TOKEN: -4}"
# The token stays in this shell only. hipc reads it from the profile that `auth login` stores
# in the temporary HOME, so no hipc process gets it from the environment or argv.
unset HIPPIUS_TOKEN HIPPIUS_TOKEN_ORIGIN HIPPIUS_ACCOUNT HIPPIUS_PROJECT HIPPIUS_BILLING
unset HIPPIUS_API_URL
unset HIPPIUS_PROFILE HIPPIUS_CONFIG
unset AWS_CONFIG_FILE AWS_SHARED_CREDENTIALS_FILE CI AI_AGENT

# Port 9 is the discard port: nothing should listen there, so a connection is refused. Plain
# http is accepted only for a loopback host, which this is. Something that does listen there
# would receive the fake token and every offline request, so the run checks first, with
# bash's /dev/tcp, which connects and sends nothing, and stops if the connection is taken.
DEAD_ORIGIN="http://127.0.0.1:9"
if (exec 3<>/dev/tcp/127.0.0.1/9) 2>/dev/null; then
  echo "release-check: something listens on 127.0.0.1:9, the origin the offline checks need" \
    "closed; stop it and run again" >&2
  exit 2
fi
FAKE_TOKEN="release-check-fake-token-0000"
FAKE_DNS_TOKEN="release-check-fake-dns-token-0000"
FAKE_APP_SECRET="release-check-fake-app-secret-0000"
ORIGIN="https://api.hippius.com"
MISSING_UUID="0f8fad5b-d9cb-469f-a165-70867728950e"
MISSING_DB="999999999"
MISSING_ZONE="z000000000000"
MISSING_LIST="hip_release_check_missing"
MISSING_FLAVOR="hipc-release-check-missing"
MISSING_BUCKET="hipc-release-check-missing"
MISSING_KEY="999999999"
MISSING_TICKET="999999999"

WORK="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/hipc-release-check.XXXXXX")"
OUT="$WORK/out"
export HOME="$WORK/home"
mkdir -m 700 "$HOME" "$OUT"

cleanup() {
  # $WORK holds secrets: the stored token in the hipc config, and the S3 secret that
  # `s3 configure` writes into the AWS files. It is removed with `rm`, never moved to a
  # trash folder, which keeps every file it receives (and backups copy it). A logout first
  # does not make `trash` safe: it clears only the config, and it can fail.
  rm -r -- "$WORK"
}
trap cleanup EXIT

pass=0
fail=0
skip=0
n=0
# The accounts the caller owns and is a member of, which account_checks reads for the
# groups after it.
OWN=""
MEMBER=""

result() {
  local status="$1" name="$2" detail="$3"
  case "$status" in
  PASS) pass=$((pass + 1)) ;;
  FAIL) fail=$((fail + 1)) ;;
  SKIP) skip=$((skip + 1)) ;;
  esac
  detail="${detail//$'\n'/ }"
  while [[ "$detail" == *"  "* ]]; do detail="${detail//  / }"; done
  printf '%s  %-62s %s\n' "$status" "$name" "$detail"
}

# The first 300 bytes of a captured file, on one line, with the token and every token
# preview replaced by ***. The whole capture is redacted before the cut, so the cut cannot
# leave part of a token.
redacted() {
  local text
  text="$(head -c 65536 -- "$1" | tr '\n\t' '  ')"
  text="${text//"$TOKEN"/***}"
  text="${text//"$PREVIEW"/***}"
  text="${text//"$FAKE_DNS_TOKEN"/***}"
  text="${text//"$FAKE_APP_SECRET"/***}"
  text="$(sed -E 's/("token_preview" *: *)"[^"]*"/\1"***"/g' <<<"$text")"
  printf '%s' "${text:0:300}"
}

# The shape of a JSON document, for a failed expectation: its keys or its type, no values.
shape() {
  [[ -s "$1" ]] || {
    echo "empty"
    return
  }
  jq -c 'if type == "object" then keys else type end' "$1" 2>/dev/null | head -c 300 ||
    echo "not JSON"
}

# The schema, read once; `effects` has one `effect<TAB>path` line per command.
"$HIPC" schema >"$WORK/schema.json"
jq -r '.commands[] | "\(.effect)\t\(.path | join(" "))"' "$WORK/schema.json" >"$WORK/effects"
cut -f2 "$WORK/effects" >"$WORK/paths"
: >"$WORK/covered"

# The command path an argv names: the longest run of words, after the global options, that
# `hipc schema` lists.
command_path() {
  while [[ $# -gt 0 && "$1" == --* ]]; do
    case "$1" in
    --output | --profile | --api-url | --account | --project | --billing | --request-timeout)
      shift 2
      ;;
    *) shift ;;
    esac
  done
  local words="" found="" word
  for word in "$@"; do
    [[ "$word" == -* ]] && break
    words="${words:+$words }$word"
    if grep -qxF -- "$words" "$WORK/paths"; then found="$words"; fi
  done
  printf '%s' "$found"
}

# Why an argv may not run online, or nothing when it may: a read_only command, or
# `hipc api GET`, without --yes or --reveal, whose --dry-run document holds only GET requests.
# --reveal is refused because a read can answer with a secret: `api GET /auth/token/
# --reveal` is a GET, yet it would print the account's master token into the CI log.
not_read_only() {
  local arg path effect
  for arg in "$@"; do
    case "$arg" in
    --yes | --reveal | --reveal=* | --api-url | --api-url=*) echo "passes $arg" && return ;;
    esac
  done
  path="$(command_path "$@")"
  effect="$(awk -F'\t' -v p="$path" '$2 == p { print $1 }' "$WORK/effects")"
  # Both probes below read hipc's output as JSON, so the output flags are left out of the
  # argv they run: --output json must not repeat or conflict with one the row passes, and
  # --debug would put debug: lines on the stderr the second probe reads.
  local dry=() drop=0
  for arg in "$@"; do
    if [[ "$drop" == 1 ]]; then drop=0 && continue; fi
    case "$arg" in
    --output) drop=1 && continue ;;
    --output=* | --json | --debug) continue ;;
    esac
    dry+=("$arg")
  done
  if [[ "$path" == api ]]; then
    [[ " $* " == *" api GET "* ]] || {
      echo "hipc api with a method other than GET" && return
    }
    # A GET that hipc confirms creates something (`GET /network/me/` provisions the
    # network) and asks before it sends, so the same argv offline, without --yes, must get
    # as far as the dead origin and fail `unavailable`. Any other outcome, a
    # `confirmation_required`, a usage error, or an exit 0 that no dead origin can give,
    # keeps it offline: the route table lives in hipc, not here, so a new row is caught too.
    HIPPIUS_TOKEN="$FAKE_TOKEN" HIPPIUS_TOKEN_ORIGIN="$DEAD_ORIGIN" "$HIPC" --api-url "$DEAD_ORIGIN" --output json "${dry[@]}" \
      >/dev/null 2>"$WORK/guard.err" </dev/null || true
    jq -e '.error.code == "unavailable"' "$WORK/guard.err" >/dev/null 2>&1 || {
      echo "its run without --yes did not fail \`unavailable\` against the dead origin" &&
        return
    }
  elif [[ "$effect" != read_only ]]; then
    echo "\`${path:-?}\` is ${effect:-not in hipc schema}" && return
  fi
  if ! HIPPIUS_TOKEN="$FAKE_TOKEN" HIPPIUS_TOKEN_ORIGIN="$DEAD_ORIGIN" "$HIPC" --api-url "$DEAD_ORIGIN" --dry-run "${dry[@]}" \
    >"$WORK/guard.json" 2>/dev/null </dev/null; then
    echo "its --dry-run failed" && return
  fi
  jq -e '[.. | objects | .method? // empty] | length > 0 and all(. == "GET")' \
    "$WORK/guard.json" >/dev/null || echo "its --dry-run shows a request that is not GET"
}

# capture <name> <expected exit> <command...>: runs the command with stdout and stderr in
# $OUT/<n>.out and $OUT/<n>.err, and checks the exit code.
capture() {
  local name="$1" want="$2" got=0
  shift 2
  n=$((n + 1))
  "$@" >"$OUT/$n.out" 2>"$OUT/$n.err" </dev/null || got=$?
  # `want` is one exit status, or several joined by `|` for a read whose answer depends on
  # the account's state.
  if [[ "|$want|" == *"|$got|"* ]]; then
    result PASS "$name" "exit $got"
  else
    result FAIL "$name" "exit $got, wanted $want: $(redacted "$OUT/$n.err")"
  fi
}

covered() { command_path "$@" >>"$WORK/covered" && echo >>"$WORK/covered"; }

# online <name> <expected exit> <hipc args...>: the real token on the real origin, read-only.
online() {
  local name="$1" want="$2" why
  shift 2
  why="$(not_read_only "$@")"
  if [[ -n "$why" ]]; then
    n=$((n + 1))
    : >"$OUT/$n.out" && : >"$OUT/$n.err"
    result FAIL "$name" "not run: $why"
    return
  fi
  covered "$@"
  capture "$name" "$want" "$HIPC" "$@"
}

# offline <name> <expected exit> <hipc args...>: a fake token, and an origin with no server.
offline() {
  local name="$1" want="$2"
  shift 2
  covered "$@"
  capture "$name" "$want" env HIPPIUS_TOKEN="$FAKE_TOKEN" HIPPIUS_TOKEN_ORIGIN="$DEAD_ORIGIN" "$HIPC" --api-url "$DEAD_ORIGIN" "$@"
}

# local_write <name> <expected exit> <hipc args...>: a command that writes only under HOME.
local_write() {
  local name="$1" want="$2" path
  shift 2
  path="$(command_path "$@")"
  case "$path" in
  "auth logout" | "s3 configure") ;;
  "billing invoices get")
    # It runs with the real token on the real origin, so it is held to the read-only guard
    # by what it sends, not by its name: its dry run, writing to a path that does not exist,
    # must list GETs alone.
    local methods
    methods="$(dry_run_methods "$@")"
    if [[ "$methods" != GET ]]; then
      n=$((n + 1))
      : >"$OUT/$n.out" && : >"$OUT/$n.err"
      result FAIL "$name" "not run: its dry run lists ${methods:-no request}, not only GET"
      return
    fi
    ;;
  *)
    n=$((n + 1))
    : >"$OUT/$n.out" && : >"$OUT/$n.err"
    result FAIL "$name" "not run: \`$path\` is not a local write"
    return
    ;;
  esac
  covered "$@"
  capture "$name" "$want" "$HIPC" "$@"
}

# dry_run_methods <hipc args...>: the methods the argv's dry run lists, each once, joined by
# commas, with any --output-file pointed at a path that does not exist; nothing when the dry
# run fails or prints no request.
dry_run_methods() {
  local arg next=0 dry=()
  for arg in "$@"; do
    if [[ "$next" == 1 ]]; then
      next=0
      dry+=("$WORK/dry-run-never-written")
      continue
    fi
    [[ "$arg" == --output-file ]] && next=1
    dry+=("$arg")
  done
  "$HIPC" --output json --dry-run "${dry[@]}" 2>/dev/null |
    jq -r '[.requests[]?.method] | unique | join(",")' 2>/dev/null || true
}

skip_check() {
  printf '%s\n' "$2" >>"$WORK/covered"
  result SKIP "$1" "$3"
}

# expect <name> <jq filter that must be true on the last stdout>
expect() {
  if jq -e "$2" "$OUT/$n.out" >/dev/null 2>&1; then
    result PASS "$1" "$2"
  else
    result FAIL "$1" "$2 on a document shaped $(shape "$OUT/$n.out")"
  fi
}

# expect_dry_run <name> <jq filter>: `expect` on the last dry run, with the fields of its last
# request, the write when there is one, beside `dry_run` and `requests`.
expect_dry_run() {
  expect "$1" "(.requests[-1] + .) | ($2)"
}

# route <command path> <method> [value...] and route_ending <command path> <method> <ending>
# [value...]: the routes of the binary under test, as `schema_route` and
# `schema_route_ending` in lib.sh give them.
route() { schema_route "$HIPC" "$@"; }
route_ending() { schema_route_ending "$HIPC" "$@"; }

# expect_requests <name> <"METHOD /path">...: the last dry run's `requests`, in order, each
# as its method and its URL without the origin.
expect_requests() {
  local name="$1" want
  shift
  want="$(jq -cn '$ARGS.positional' --args "$@")"
  expect "$name" "[.requests[] | .method + \" \" + (.url | ltrimstr(\"$DEAD_ORIGIN\"))] == $want"
}

# expect_err <name> <jq filter that must be true on the last stderr>
expect_err() {
  if jq -e "$2" "$OUT/$n.err" >/dev/null 2>&1; then
    result PASS "$1" "$2"
  else
    result FAIL "$1" "$2 on: $(redacted "$OUT/$n.err")"
  fi
}

expect_code() { expect_err "$1" ".error.code == \"$2\""; }

stderr_has() {
  if grep -qF -- "$2" "$OUT/$n.err"; then
    result PASS "$1" "stderr has '$2'"
  else
    result FAIL "$1" "stderr lacks '$2': $(redacted "$OUT/$n.err")"
  fi
}

check() {
  local name="$1" detail="$2"
  shift 2
  if "$@"; then result PASS "$name" "$detail"; else result FAIL "$name" "$detail"; fi
}

# The mode of a file, or `missing`: a bare assignment of a failed substitution would end the
# run under `set -e`, before the summary and the token scan.
mode_of() { stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1" 2>/dev/null || echo missing; }

last_out() { cat -- "$OUT/$n.out"; }

# A string from the last stdout, or nothing when it is missing or the output is not JSON.
field() { jq -r "$1 // empty" "$OUT/$n.out" 2>/dev/null || true; }

# The online guard refuses each argv here before it runs anything, so a row added later
# with one of these flags or methods fails instead of reaching the server with the token.
guard_checks() {
  echo "== the online guard (nothing is run) =="
  local argv why words
  for argv in "api GET /auth/token/ --reveal" "api GET /auth/token/ --reveal=true" \
    "account list --yes" "api POST /accounts/" "--api-url $DEAD_ORIGIN account list" \
    "api GET /network/me/" "api GET /network/me --output text" "network create" \
    "api POST /network/provision/"; do
    read -ra words <<<"$argv"
    why="$(not_read_only "${words[@]}")"
    check "online refuses $argv" "$why" test -n "$why"
  done
  # The output flags a row may carry are stripped before the probes, so a plain GET with
  # them still passes; both probes hit the dead origin with the fake token.
  for argv in "api GET /accounts/ --output text" "api GET /accounts/ --output=text" \
    "api GET /accounts/ --json --debug"; do
    read -ra words <<<"$argv"
    why="$(not_read_only "${words[@]}")"
    check "online accepts $argv" "$why" test -z "$why"
  done
}

baseline() {
  echo "== baseline (no network) =="
  offline "--help" 0 --help
  check "--help shows usage" "Usage: hipc" grep -q "Usage: hipc" "$OUT/$n.out"
  local path
  while IFS= read -r path; do
    # shellcheck disable=SC2086 # the path's words are separate arguments
    capture "$path --help" 0 "$HIPC" $path --help
  done <"$WORK/paths"
  offline "-V" 0 -V
  check "-V prints hipc $VERSION" "" test "$(last_out)" = "hipc $VERSION"
  offline "version" 0 version
  expect "version and target" ".version == \"$VERSION\" and .target == \"$TARGET\""
  offline "schema" 0 schema
  expect "schema effects" '.schema_version == 4 and (.commands | length > 0) and
    all(.commands[]; .effect | IN("read_only", "idempotent", "non_idempotent"))'
  expect "schema arguments and flags" '(.global_flags | map(.name) | index("--yes") != null)
    and all(.commands[]; (.args | type) == "array" and (.flags | type) == "array")
    and any(.commands[]; .path == ["api"] and (.args | map(.name)) == ["method", "path"])'
  expect "schema api guards" 'all(.commands[]; has("route_flags") | not) and
    any(.commands[]; .path == ["api"] and (.guards | map(.code)) ==
      ["change_isolation_required", "public_required", "remove_others_required"])'
  local shell
  for shell in bash zsh fish elvish powershell; do
    offline "completions $shell" 0 completions "$shell"
  done
  capture "bare hipc" 2 "$HIPC"
  check "bare hipc stdout is empty" "" test ! -s "$OUT/$n.out"
  expect_err "bare hipc is usage" '.error.code == "usage" and (.error.message | length > 0)'
  capture "--bogus" 2 "$HIPC" --bogus
  expect_err "--bogus is usage" '.error.code == "usage" and (.error.message | test("--bogus"))'
  local status
  status="$(
    "$HIPC" completions powershell | head -c 10 >/dev/null
    echo "${PIPESTATUS[0]}"
  )"
  check "closed pipe exits 0" "exit $status" test "$status" = 0
}

auth_checks() {
  echo "== auth (local writes under a temporary HOME) =="
  online "auth status without a token" 1 auth status
  expect_code "no token is auth" auth
  n=$((n + 1))
  local got=0
  printf '%s\n' "$TOKEN" | "$HIPC" auth login --token-file - >"$OUT/$n.out" 2>"$OUT/$n.err" ||
    got=$?
  printf 'auth login\n' >>"$WORK/covered"
  check "auth login from stdin" "exit $got" test "$got" = 0
  expect "token stored for the default origin" \
    ".token_stored == true and .profile == \"default\" and .token_origin == \"$ORIGIN\""
  local file dir
  file="$(mode_of "$HOME/.hippius/config.toml")"
  dir="$(mode_of "$HOME/.hippius")"
  check "config modes" "file $file, dir $dir" test "$file:$dir" = 600:700
  # No HIPPIUS_TOKEN: the stored token is withheld from another origin, and nothing is sent.
  capture "stored token withheld from another origin" 1 \
    "$HIPC" --api-url "$DEAD_ORIGIN" auth status
  # shellcheck disable=SC2016 # the backticks are part of hipc's message, not a substitution
  expect_err "withheld is auth" \
    '.error.code == "auth" and
      (.error.message | contains("stored token is for `https://api.hippius.com`"))'
  # shellcheck disable=SC2016 # the backticks are part of hipc's hint, not a substitution
  expect_err "withheld hints the flag back to its origin" \
    '.error.hint | startswith("pass `--api-url https://api.hippius.com`")'
  # A profile the file lacks is named, and a name outside the rules is refused; both before
  # anything is sent, and against the dead origin regardless.
  capture "a profile the file lacks is named" 1 \
    "$HIPC" --api-url "$DEAD_ORIGIN" --profile nope auth status
  expect_err "unknown profile is auth" \
    '.error.code == "auth" and (.error.message | contains("has no profile"))'
  capture "a bad profile name is usage" 2 \
    "$HIPC" --api-url "$DEAD_ORIGIN" --profile 'a b' auth status
  expect_code "bad profile name" usage
  online "auth status" 0 auth status
  expect "status shape" '(has("ok") | not) and .token_source == "config" and
    (.token_preview | length) == 4'
  # The signed-in user: a failed read keeps exit 0 and sets user_error, so this
  # is where a shape `hipc` does not read shows up.
  expect "status names the user" '(.user.username | type) == "string" and
    (has("user_error") | not)'
  online "auth status --debug" 0 auth status --debug
  stderr_has "debug shows the URL" \
    "debug: GET $ORIGIN$(route_ending "auth status" GET /accounts/)"
  stderr_has "debug shows the user read" "debug: GET $ORIGIN$(route_ending "auth status" GET /me/)"
}

account_checks() {
  echo "== account and billing (live, read-only) =="
  online "status" 0 status
  expect "status is an object" 'type == "object"'
  online "account list" 0 account list
  expect "accounts envelope" '(.items | length) >= 1 and .next_page_token == null'
  OWN="$(field '[.items[] | select(.is_owner == true)][0].id')"
  MEMBER="$(field '[.items[] | select(.is_owner == false)][0].id')"
  online "account get" 0 account get
  check "get is the own account" "" test "$(field .id)" = "$OWN"
  online "account notifications get" 0 account notifications get
  expect "notification settings are an object" 'type == "object"'
  online "account marketing-consent get" 0 account marketing-consent get
  expect "marketing consent has its state" \
    '(.state | IN("yes", "no", "never_asked")) and (.receives_marketing | type) == "boolean"'
  online "account members list" 0 account members list
  expect "members envelope" 'has("items") and has("next_page_token")'
  online "account invites list" 0 account invites list
  expect "invites envelope" 'has("items") and has("next_page_token")'
  # The referral links and analytics are the signed-in user's; a create or a rename runs
  # offline only, since a create mints the user's referral code on first use.
  online "account referrals list" 0 account referrals list
  expect "referrals envelope" '(.items | type) == "array" and .next_page_token == null'
  online "account referrals analytics" 0 account referrals analytics
  expect "referral analytics are an object" 'type == "object"'
  # The project and billing account lists, read in the Default project without a flag.
  online "account projects list" 0 account projects list
  expect "projects envelope" '(.items | type) == "array" and .next_page_token == null'
  local project_id
  project_id="$(field '[.items[] | select(.id != null)][0].id')"
  if [[ -n "$project_id" ]]; then
    online "project budget get" 0 account projects budget get "$project_id"
    expect "budget has a limit field" 'has("monthly_limit")'
    online "project quota get" 0 account projects quota get "$project_id"
    expect "quota has limits" '(.limits | type) == "object"'
  else
    skip_check "project budget get" "account projects budget get" "the account has no project"
    skip_check "project quota get" "account projects quota get" "the account has no project"
  fi
  # The transfers are personal: the token's own account's, whichever account is selected.
  online "account project-transfers list" 0 account project-transfers list
  expect "transfers envelope" '(.items | type) == "array" and .next_page_token == null and
    (.items | all(.direction == "incoming" or .direction == "outgoing"))'
  online "account billing-accounts list" 0 account billing-accounts list
  expect "billing accounts envelope" '(.items | type) == "array" and .next_page_token == null'
  online "vms in the Default project" 0 --project default compute vms list
  expect "default sends no project" 'type == "object"'
  online "api GET /accounts/" 0 api GET /accounts/
  expect "raw accounts" '.accounts | length >= 1'
  online "api GET --all /ssh-keys/" 0 api GET /ssh-keys/ --all
  expect "every page as one array" 'type == "array"'
  online "billing balance" 0 billing balance
  expect "balance" 'has("balance")'
  online "billing balance in text mode" 0 --output text billing balance
  check "text block" "" grep -q '^balance ' "$OUT/$n.out"
  # The hAlpha quote is a read; the buy stays offline below.
  online "billing native-quote" 0 billing native-quote
  expect "the quote names its rail state" \
    '(.enabled | type) == "boolean" and (.stale | type) == "boolean" and (.custodial | type) == "boolean"'
  expect "the quote keeps amounts as exact strings" \
    '[.alpha_price, .min_alpha, .native_balance] | all(type == "string" and test("^[0-9]+$"))'
  online "billing subscriptions" 0 billing subscriptions
  expect "four plan sources in order" '[.items[].service] == ["drive", "s3", "hub", "stripe"]'
  online "billing transactions --limit 2" 0 billing transactions --limit 2
  expect "transactions page keeps count" \
    'has("next_page_token") and has("count") and (.items | length) <= 2'
  online "billing transactions --all" 0 billing transactions --all
  expect "transactions --all is an array" 'type == "array"'
  online "billing stripe" 0 billing stripe
  expect "stripe document" 'has("has_subscription")'
  online "billing summary" 0 billing summary
  expect "compute summary" 'has("burn_rate_per_hour") and has("runway_hours")'
  online "billing summary in text mode" 0 --output text billing summary
  # The note's verb follows how many fields it converted ("is" for one, "are" for several),
  # and that count depends on the account, so match only the part every note has.
  stderr_has "summary amounts in credits" "in credits here (1 credit = 1 USD)"
  online "billing usage" 0 billing usage
  expect "usage month" 'has("month") and (.hours | type) == "array"'
  online "billing usage --month --debug" 0 billing usage --month 2026-09 --debug
  stderr_has "usage URL" \
    "debug: GET $ORIGIN$(route "billing usage" GET)?month=<redacted>"
  online "billing auto-reload get" 0 billing auto-reload get
  expect "auto-reload document" 'has("available")'
  online "billing auto-reload history --limit 5" 0 billing auto-reload history --limit 5
  expect "auto-reload attempts" '(.results | type) == "array" and (.results | length) <= 5'
  # The profile update's flags are named for these fields, so a live run shows whether the
  # field names hipc assumes (the console's form; the API documents no body) hold.
  online "billing profile get" 0 billing profile get
  expect "billing profile has the fields the update's flags set" \
    '[.company_name, .address_line1, .address_line2, .city, .postal_code, .region, .country,
      .tax_id, .billing_email] | all(type == "string")'
  online "billing invoices list --limit 1" 0 billing invoices list --limit 1
  expect "invoices page" 'has("next_page_token") and (.items | length) <= 1'
  invoice_checks "$(field '.items[0].id')"
}

# The invoice PDF goes to a new file in $WORK, which the script deletes on exit.
invoice_checks() {
  local invoice="$1" pdf="$WORK/invoice.pdf" missing="$WORK/missing.pdf" mode
  local_write "invoice of a missing id" 1 billing invoices get 999999999 --output-file "$missing"
  expect_code "missing invoice is not_found" not_found
  check "a failed download writes no file" "" test ! -e "$missing"
  if [[ -z "$invoice" ]]; then
    result SKIP "billing invoices get" "the account has no invoice"
    return
  fi
  local_write "billing invoices get" 0 billing invoices get "$invoice" --output-file "$pdf"
  expect "download document" ".bytes > 0 and .output_file == \"$pdf\""
  mode="$(mode_of "$pdf")"
  check "invoice file mode" "$mode" test "$mode" = 600
  check "invoice is a PDF" "" test "$(head -c 5 -- "$pdf")" = "%PDF-"
  local_write "invoice get again" 2 billing invoices get "$invoice" --output-file "$pdf"
  expect_code "an existing file is usage" usage
}

# jq definitions for the Hub robot and key checks. `secret_keys` is whether an object has a
# key hipc drops as a secret: one of the four names in any case, or one that holds `password`.
# `hipc_docker_login` is the command hipc builds, with the login as one word the way
# `phrase::shell_word` writes it: plain when every byte is one a shell leaves alone, else
# single-quoted, a quote inside written as quote, backslash, quote, quote. The regex writes
# a single quote as `\x27`, since a shell single-quoted string cannot hold one.
HIPC_SECRET_KEYS='def secret_keys: keys | map(ascii_downcase)
  | any(IN("secret", "robot_secret", "token") or test("password"));'
HIPC_DOCKER_LOGIN='def hipc_docker_login: test("^docker login registry\\.hippius\\.com -u ([A-Za-z0-9_./:=@%+,-]+|\\x27([^\\x27]|\\x27\\\\\\x27\\x27)*\\x27) --password-stdin$");'

storage_checks() {
  echo "== drive, s3, hub (live, read-only) =="
  online "drive plans" 0 drive plans
  expect "drive catalog" 'any(.items[]; .code == "solo")'
  online "drive subscription get" 0 drive subscription get
  expect "drive subscription" 'has("active")'
  online "drive subscription history" 0 drive subscription history --limit 5
  expect "drive history page" 'has("next_page_token") and (.items | length) <= 5'
  online "drive checkout of a missing intent" 1 drive subscription checkout "$MISSING_UUID"
  expect_code "missing drive checkout is not_found" not_found
  online "s3 plans" 0 s3 plans
  expect "s3 catalog" '(.plans | length) > 0'
  # A plan purchase or change checks its code against plans[].code before it buys.
  expect "each s3 plan has the code a purchase checks" 'all(.plans[]; (.code | type) == "string")'
  online "s3 subscription get" 0 s3 subscription get
  expect "s3 mode" 'has("mode")'
  online "s3 subscription history" 0 s3 subscription history --limit 5
  expect "s3 history page" 'has("next_page_token") and (.items | length) <= 5'
  online "s3 checkout of a missing intent" 1 s3 subscription checkout "$MISSING_UUID"
  expect_code "missing s3 checkout is not_found" not_found
  online "s3 buckets list" 0 s3 buckets list
  expect "buckets envelope" 'has("items") and .next_page_token == null'
  local bucket
  bucket="$(field '.items[0].name')"
  # The API documents only a 400 for a bucket the policy read cannot serve, so a missing
  # bucket may answer either.
  online "s3 buckets acl list" 0 s3 buckets acl list
  expect "acl view envelope" 'has("items") and all(.items[]; has("bucket"))'
  online "s3 buckets acl get on a missing bucket" 1 s3 buckets acl get "$MISSING_BUCKET"
  expect_err "missing bucket acl is not_found or validation" \
    '.error.code | IN("not_found", "validation")'
  # The API documents a 404 here, but the live answer to a missing bucket is unverified, so
  # a 400 passes too, as for the policy read.
  online "s3 buckets objects list on a missing bucket" 1 \
    s3 buckets objects list "$MISSING_BUCKET"
  expect_err "missing bucket objects is not_found or validation" \
    '.error.code | IN("not_found", "validation")'
  if [[ -n "$bucket" ]]; then
    online "s3 buckets acl get" 0 s3 buckets acl get "$bucket"
    expect "bucket policy" 'has("bucket") and has("policy")'
    online "s3 buckets objects list" 0 s3 buckets objects list "$bucket" --limit 1
    expect "objects page" \
      'has("next_page_token") and (.items | length) <= 1 and all(.items[]; has("Key"))'
  else
    result SKIP "s3 buckets acl get" "the account has no bucket"
    result SKIP "s3 buckets objects list" "the account has no bucket"
  fi
  online "s3 tokens list" 0 s3 tokens list
  expect "token kinds" 'all(.items[]; .kind == "master" or .kind == "sub")'
  expect "no secret in a listing" 'all(.items[]; .secret == null)'
  online "hub plans" 0 hub plans
  expect "hub catalog" '(.items | length) > 0 and all(.items[]; has("id") and has("name"))'
  online "hub plans --debug" 0 hub plans --debug
  stderr_has "hub plans URL" "debug: GET $ORIGIN$(route "hub plans" GET)"
  online "hub subscription get" 0 hub subscription get
  expect "hub subscriptions envelope" 'has("items") and .next_page_token == null'
  online "hub namespace availability" 0 hub namespace availability hipc-release-check-zz
  expect "namespace availability" 'has("available")'
  online "hub namespace status" 0 hub namespace status
  hub_project_read "hub namespace get" "no robot secret" \
    '[.. | objects | has("robot_secret")] | any | not' hub namespace get
  # The API documents no secret in the robot read. hipc drops every key a secret or a
  # `docker login` command may sit under (the four secret names in any case, any key holding
  # `password`, and the command keys) and refuses an answer with any other `docker login`
  # command, so the only one left is `docker_login_command`, which hipc builds and which
  # reads the secret from stdin. The password-flag scan is a second net under that rule.
  hub_project_read "hub namespace robot get" "no secret in the robot login" \
    "$HIPC_SECRET_KEYS$HIPC_DOCKER_LOGIN"'
      ([.. | objects | secret_keys] | any | not) and
      ([.. | strings | splits("\\s+") | test("^(-p|--password$|--password=)")] | any | not) and
      ([del(.docker_login_command) | .. | strings | test("docker\\s+login"; "i")]
        | any | not) and
      (.docker_login_command == null or (.docker_login_command | hipc_docker_login))' \
    hub namespace robot get
  hub_project_read "hub usage" "hub usage object" 'type == "object"' hub usage
  hub_project_read "hub usage --repositories" "hub usage envelope" \
    'has("items") and .next_page_token == null' hub usage --repositories
  # A key list holds the same as the robot read, row by row, with `secret: null` (tenet 9).
  hub_project_read "hub keys list" "no secret in the key list" \
    "$HIPC_SECRET_KEYS$HIPC_DOCKER_LOGIN"'
      has("items") and
      all(.items[]; .secret == null and
        (.docker_login_command == null or (.docker_login_command | hipc_docker_login))) and
      ([.items[] | del(.secret) | .. | objects | secret_keys] | any | not) and
      ([.items[] | .. | strings | splits("\\s+") | test("^(-p|--password$|--password=)")]
        | any | not) and
      ([.items[] | del(.docker_login_command) | .. | strings | test("docker\\s+login"; "i")]
        | any | not)' \
    hub keys list
  hub_key_read
  hub_project_read "hub events list" "hub events envelope" \
    'has("items") and .next_page_token == null' hub events list
  hub_project_read "hub repositories list" "hub repositories envelope" \
    'has("items") and .next_page_token == null' hub repositories list
  hub_repository_reads
  hub_model_reads
}

# The read of one key, the first the key list above holds, as the list prints it. An account
# with none, or with no Hub project, skips it; nothing here creates a key.
hub_key_read() {
  local key_id
  key_id="$(field '.items[0].id')"
  if [[ -z "$key_id" ]]; then
    skip_check "hub keys get" "hub keys get" "the account lists no Hub key"
    return
  fi
  hub_project_read "hub keys get" "no secret in the key" \
    "$HIPC_SECRET_KEYS$HIPC_DOCKER_LOGIN"'
      .secret == null and
      (.docker_login_command == null or (.docker_login_command | hipc_docker_login)) and
      ([del(.secret) | .. | objects | secret_keys] | any | not) and
      ([del(.docker_login_command) | .. | strings | test("docker\\s+login"; "i")]
        | any | not)' \
    hub keys get -- "$key_id"
}

# The model index is public, so every account reads it. A model and a version come from the
# first page; an empty index skips them.
hub_model_reads() {
  local project repo digest none="the Hub model index is empty"
  online "hub models list" 0 hub models list --limit 2
  expect "hub models page" \
    'has("items") and has("total") and (.items | length) <= 2 and
      (.next_page_token == null or (.next_page_token | test("^[0-9]+$")))'
  project="$(field '.items[0].project')"
  repo="$(field '.items[0].repo')"
  digest="$(field '.items[0].digest')"
  online "hub models formats" 0 hub models formats
  expect "hub model formats" '(.formats | type) == "array" and (.architectures | type) == "array"'
  if [[ -z "$project" || -z "$repo" ]]; then
    skip_check "hub models get" "hub models get" "$none"
    return
  fi
  online "hub models get" 0 hub models get --limit 2 -- "$project/$repo"
  expect "hub model versions" \
    ".repo == $(jq -cn --arg v "$repo" '$v') and (.items | length) <= 2"
  if [[ -n "$digest" ]]; then
    online "hub models get --reference" 0 hub models get --reference "$digest" -- "$project/$repo"
    expect "hub model version by digest" ".digest == $(jq -cn --arg v "$digest" '$v')"
  fi
}

# The reads of one repository and one artifact, the first ones the list above holds. An
# account with none skips them; nothing here deletes, so the release check never makes one.
# Each name goes after `--`, so a name that starts with `-` is not read as a flag.
hub_repository_reads() {
  local repository reference none="the account lists no Hub repository"
  repository="$(field '.items[0].name')"
  if [[ -z "$repository" ]]; then
    skip_check "hub repositories get" "hub repositories get" "$none"
    skip_check "hub artifacts list" "hub artifacts list" "$none"
    skip_check "hub artifacts get" "hub artifacts get" "$none"
    return
  fi
  online "hub repositories get" 0 hub repositories get -- "$repository"
  # The names are written into the filter as JSON strings, so no character in them is jq.
  expect "hub repository by name" ".name == $(jq -cn --arg v "$repository" '$v')"
  online "hub artifacts list" 0 hub artifacts list -- "$repository"
  expect "hub artifacts envelope" 'has("items") and .next_page_token == null'
  reference="$(field '.items[0].digest')"
  if [[ -z "$reference" ]]; then
    skip_check "hub artifacts get" "hub artifacts get" "the first Hub repository is empty"
    return
  fi
  online "hub artifacts get" 0 hub artifacts get -- "$repository" "$reference"
  expect "hub artifact by digest" ".digest == $(jq -cn --arg v "$reference" '$v')"
}

# hub_project_read <name> <check name> <jq filter> <hipc args...>: a read of the account's
# Hub project. An account that never provisioned one gets 403 `You must call
# /api/registry/provision/ first.`: account state, like a missing VM. hipc names the
# provision command in its hint then, and the check is skipped. Any other failure still
# fails it.
hub_project_read() {
  local name="$1" check_name="$2" filter="$3"
  shift 3
  online "$name" "0|1" "$@"
  if jq -e '.error.http_status == 403' "$OUT/$n.err" >/dev/null 2>&1; then
    expect_err "$name without a Hub project hints at namespace create" \
      '.error.hint | test("hipc hub namespace create")'
    result SKIP "$check_name" "the account has no Hub project"
  else
    expect "$check_name" "$filter"
  fi
}

compute_catalog_checks() {
  echo "== compute (live, read-only) =="
  online "compute prices" 0 compute prices
  expect "prices object" 'has("currency") and (has("items") | not)'
  online "compute public-prices" 0 compute public-prices
  expect "public prices in credits" '.currency == "credits" and has("os_disk_tiers")'
  online "compute quota" 0 compute quota
  expect "quota runway" 'has("runway")'
  online "compute charge-cap" 0 compute charge-cap
  expect "charge cap object" 'has("charge_cap") and has("request")'
  online "compute images" 0 compute images
  expect "images object" 'has("images") and has("gateway_available")'
  online "compute flavors" 0 compute flavors
  expect "flavors envelope" '(.items | length) > 0 and all(.items[]; has("code"))'
  local flavor
  flavor="$(field '.items[0].code')"
  online "compute feasibility of a missing flavor" 1 compute feasibility --flavor "$MISSING_FLAVOR"
  expect_code "missing flavor is not_found" not_found
  if [[ -n "$flavor" ]]; then
    online "compute feasibility" 0 compute feasibility --flavor "$flavor"
    expect "feasibility verdict" 'has("verdict") and (.feasible | type) == "boolean"'
    online "compute feasibility on hdd" 0 compute feasibility --flavor "$flavor" \
      --os-disk-tier hdd
    expect "feasibility echoes the tier" '.os_disk_tier == "hdd" and has("verdict")'
  else
    result SKIP "compute feasibility" "the flavor list is empty"
  fi
  online "compute apps" 0 compute apps
  expect "apps envelope" 'has("items") and .next_page_token == null'
  online "compute regions" 0 compute regions
  expect "regions envelope" '(.items | length) > 0 and .next_page_token == null'
  online "compute placement-groups" 0 compute placement-groups
  expect "placement groups" 'has("available")'
  online "compute public-ip availability" 0 compute public-ip availability
  expect "public IP availability" 'has("regions")'
  online "compute backups options" 0 compute backups options
  expect "backup options" 'has("intervals")'
  # A backup policy update checks its interval against intervals[].seconds before it sends.
  expect "each backup interval has the seconds a policy update checks" \
    'all(.intervals[]; (.seconds | type) == "number")'
  online "compute runners settings get" 0 compute runners settings get
  expect "runner settings" 'has("enabled")'
  online "compute runners installations list" 0 compute runners installations list
  expect "installations envelope" 'has("items")'
  online "compute runners jobs" 0 compute runners jobs
  expect "jobs envelope" 'has("items") and has("next_page_token")'
  online "compute runners usage" 0 compute runners usage
  expect "runner usage object" 'type == "object"'
  # The limit answers 404 while the server's runner usage limits are off.
  online "compute runners limit get" "0|1" compute runners limit get
  if [[ -s "$OUT/$n.err" ]]; then
    expect_err "runner limit off is not_found with its hint" \
      '.error.code == "not_found" and (.error.hint | test("not enabled on this server"))'
  else
    expect "runner limit has its cap in credits" 'has("monthly_cap_credits")'
    # limit update reads monthly_cap too, a name no API document gives.
    expect "runner limit names its base-unit cap monthly_cap" 'has("monthly_cap")'
  fi
  online "compute ssh-keys list" 0 compute ssh-keys list
  expect "ssh keys envelope" 'has("items") and has("next_page_token")'
  online "compute ssh-keys list --all" 0 compute ssh-keys list --all
  expect "ssh keys --all is an array" 'type == "array"'
  local key
  key="$(field '.[0].id')"
  online "compute ssh-keys get of a missing key" 1 compute ssh-keys get "$MISSING_KEY"
  expect_code "missing ssh key is not_found" not_found
  if [[ -n "$key" ]]; then
    online "compute ssh-keys get" 0 compute ssh-keys get "$key"
    expect "one ssh key" "(.id | tostring) == \"$key\""
  else
    result SKIP "compute ssh-keys get" "the account has no SSH key"
  fi
  online "compute account-ssh-keys" 0 compute account-ssh-keys
  expect "account ssh keys envelope" 'has("items") and .next_page_token == null'
  online "compute databases refusals" 0 compute databases refusals
  expect "database refusals" 'has("refusals")'
  online "cdn prices" 0 cdn prices
  expect "cdn prices" 'has("regions")'
  online "cdn regions" 0 cdn regions
  expect "cdn regions" 'has("regions")'
}

# Every route under a VM, once on an id that does not exist (404, which needs no state), and
# once on the account's first VM when it has one.
vm_checks() {
  local reads=(
    "compute vms get"
    "compute vms attestation"
    "compute vms failovers"
    "compute vms resize-options"
    "compute vms power-policy get"
    "compute public-ip get"
    "compute public-ip usage"
    "compute firewall get"
    "compute exposures list"
    "compute backups policy get"
    "compute backups list"
    "compute backups restore-status"
  )
  online "compute vms list --debug" 0 compute vms list --debug
  stderr_has "vms URL" "debug: GET $ORIGIN$(route "compute vms list" GET)"
  expect "vms envelope" 'has("items") and .next_page_token == null'
  local vm read
  vm="$(field '.items[0].vm_id')"
  for read in "${reads[@]}"; do
    # shellcheck disable=SC2086 # the command's words are separate arguments
    online "$read on a missing VM" 1 $read "$MISSING_UUID"
    expect_code "$read on a missing VM is not_found" not_found
    if [[ -n "$vm" ]]; then
      # shellcheck disable=SC2086 # the command's words are separate arguments
      online "$read" 0 $read "$vm"
    else
      result SKIP "$read" "the account has no VM"
    fi
  done
  online "a missing resize" 1 compute vms resize-status "$MISSING_UUID" 1
  expect_code "missing resize is not_found" not_found
}

database_checks() {
  online "compute databases list" 0 compute databases list
  expect "databases envelope" 'has("items")'
  local db read
  db="$(field '.items[0].id')"
  for read in "compute databases get" "compute databases events" \
    "compute databases metrics"; do
    # shellcheck disable=SC2086 # the command's words are separate arguments
    online "$read on a missing database" 1 $read "$MISSING_DB"
    expect_code "$read on a missing database is not_found" not_found
    if [[ -n "$db" ]]; then
      # shellcheck disable=SC2086 # the command's words are separate arguments
      online "$read" 0 $read "$db"
    else
      result SKIP "$read" "the account has no database"
    fi
  done
  # The delete question and note name the backup bucket and token the server makes for a
  # database by a pattern seen once live; these rows show whether it still holds.
  if [[ -n "$db" ]]; then
    online "s3 buckets list, for the backup bucket of a database" 0 s3 buckets list
    expect "the database's backup bucket is named hippius-db-<id>-backups" \
      "any(.items[]; .name == \"hippius-db-$db-backups\")"
    online "s3 tokens list, for the backup token of a database" 0 s3 tokens list
    expect "the database's backup token is named db-<id>-backup-writer" \
      "any(.items[]; .name == \"db-$db-backup-writer\")"
  else
    result SKIP "database backup bucket and token names" "the account has no database"
  fi
}

# Every Kubernetes read, once on a cluster id that does not exist (404, which needs no state),
# and once on the account's first cluster when it has one; then the local refusals.
kubernetes_checks() {
  echo "== kubernetes (live, read-only) =="
  online "compute kubernetes clusters list" 0 compute kubernetes clusters list
  expect "clusters envelope" \
    'has("items") and has("next_page_token") and .next_page_token == null'
  local cluster read
  cluster="$(field '.items[0].id')"
  online "compute kubernetes clusters refusals" 0 compute kubernetes clusters refusals
  expect "cluster refusals envelope" \
    '(.items | type) == "array" and has("next_page_token") and .next_page_token == null'
  # Backups needs --reveal, so it runs only offline, in the dry-run table: its answer holds
  # presigned download links.
  for read in "compute kubernetes clusters get" "compute kubernetes clusters events" \
    "compute kubernetes node-pools list" "compute kubernetes operations list"; do
    # shellcheck disable=SC2086 # the command's words are separate arguments
    online "$read on a missing cluster" 1 $read "$MISSING_DB"
    expect_code "$read on a missing cluster is not_found" not_found
    if [[ -n "$cluster" ]]; then
      # shellcheck disable=SC2086 # the command's words are separate arguments
      online "$read" 0 $read "$cluster"
      if [[ "$read" == "compute kubernetes clusters events" ]]; then
        expect "cluster events envelope" \
          '(.items | type) == "array" and has("next_page_token") and .next_page_token == null'
      fi
      # A scale signs as many adds as the console's projection of the pool's size counts,
      # from these fields; the server is not known to count the same way.
      if [[ "$read" == "compute kubernetes node-pools list" ]]; then
        expect "each pool has the fields the scale projection counts" \
          'all(.items[]; has("name") and all((.nodes // [])[]; has("name") and has("state")))'
      fi
      if [[ "$read" == "compute kubernetes operations list" ]]; then
        expect "each operation has the fields the scale projection counts" \
          'all(.items[]; has("kind") and has("state"))'
      fi
    else
      result SKIP "$read" "the account has no Kubernetes cluster"
    fi
  done
  offline "cluster id not a number" 2 compute kubernetes clusters get x
  expect_err "a cluster id names its list" \
    '.error.message | test("from .hipc compute kubernetes clusters list.")'
  offline "node name with a dot segment" 2 compute kubernetes nodes delete 7 .. --yes
  expect_err "a dot segment is not a node name" \
    '.error.code == "usage" and (.error.message | test("a node name is"))'
  offline "node name with a slash" 2 compute kubernetes nodes delete 7 a/b --yes
  offline "operation id not a number" 2 compute kubernetes operations resume 7 x --yes
  offline "label key given twice" 2 compute kubernetes node-pools create 7 --name w \
    --flavor medium --label a=b --label a=c --yes
  expect_err "a repeated label key is usage" \
    '.error.code == "usage" and (.error.message | test("more than once"))'
  kubernetes_key_checks
}

# App Engine: the limits, the list, and every read under an app, once on an id that does not
# exist and once on the account's first app. Outside the feature every route but `get`
# answers 403 `app-engine-unavailable`, which is `permission` with a hint naming `get`.
app_engine_checks() {
  echo "== app engine (live, read-only) =="
  local reads=(
    "compute app-engine apps get"
    "compute app-engine apps runtime"
    "compute app-engine guest-components get"
    "compute app-engine guest-components update-status"
  )
  online "compute app-engine get" 0 compute app-engine get
  expect "app engine limits" '(.enabled | type) == "boolean"'
  local enabled app read
  enabled="$(field '.enabled')"
  if [[ "$enabled" != true ]]; then
    online "compute app-engine apps list outside the feature" 1 compute app-engine apps list
    expect_err "outside the feature names get" \
      '.error.code == "permission" and (.error.hint | test("app-engine get"))'
    for read in "${reads[@]}"; do
      # shellcheck disable=SC2086 # the command's words are separate arguments
      online "$read outside the feature" 1 $read "$MISSING_UUID"
      expect_code "$read outside the feature is permission" permission
    done
    return
  fi
  online "compute app-engine apps list" 0 compute app-engine apps list
  expect "apps envelope" '(.items | type) == "array" and .next_page_token == null'
  app="$(field '.items[0].vm_id')"
  for read in "${reads[@]}"; do
    # shellcheck disable=SC2086 # the command's words are separate arguments
    online "$read on a missing app" 1 $read "$MISSING_UUID"
    expect_code "$read on a missing app is not_found" not_found
    if [[ -n "$app" ]]; then
      # shellcheck disable=SC2086 # the command's words are separate arguments
      online "$read" 0 $read "$app"
    else
      result SKIP "$read" "the account has no App Engine app"
    fi
  done
}

# The commands that take a cluster key file run only offline: no key file is ever used against
# the live API. The key is the console's public test vector (seeds 11..42 and 1..32), never a
# real one, in a private file under $WORK.
kubernetes_key_checks() {
  local key="$WORK/hippius-cluster-check.key" open="$WORK/open.key"
  (
    umask 077
    write_test_cluster_key "$key"
  )
  cp -- "$key" "$open" && chmod 644 "$open"
  offline "kubeconfig --dry-run" 0 --dry-run compute kubernetes clusters kubeconfig 7 \
    --cluster-key-file "$key" --output-file "$WORK/kubeconfig"
  local kubeconfig="compute kubernetes clusters kubeconfig"
  expect_requests "kubeconfig reads the cluster, then its secrets" \
    "GET $(route_ending "$kubeconfig" GET "{cluster_id}/" 7)" \
    "GET $(route_ending "$kubeconfig" GET /secrets/ 7)"
  check "a dry run writes no kubeconfig" "" test ! -e "$WORK/kubeconfig"
  offline "node-pools scale --dry-run" 0 --dry-run compute kubernetes node-pools scale 7 3 \
    --count 2 --cluster-key-file "$key"
  local scale="compute kubernetes node-pools scale" cluster pools operations
  cluster="GET $(route_ending "$scale" GET "{cluster_id}/" 7)"
  pools="GET $(route_ending "$scale" GET /node-pools/ 7)"
  operations="GET $(route_ending "$scale" GET /operations/ 7)"
  expect_requests "scale reads the cluster, its pools, and its operations, the last two twice" \
    "$cluster" "$pools" "$operations" "$pools" "$operations" \
    "POST $(route "$scale" POST 7 3)"
  expect "scale signs nothing in a dry run" \
    '.requests[5].body.count == 2 and (.requests[5].body.operations | test("^<"))'
  offline "nodes replace --dry-run" 0 --dry-run compute kubernetes nodes replace 7 work-1 \
    --cluster-key-file "$key"
  expect_requests "replace reads the cluster, its pools, and its operations, the last two twice" \
    "$cluster" "$pools" "$operations" "$pools" "$operations" \
    "POST $(route_ending "compute kubernetes nodes replace" POST /replace/ 7 work-1)"
  offline "key file open to others" 2 compute kubernetes nodes replace 7 work-1 \
    --cluster-key-file "$open" --yes
  expect_err "an open key file is usage with a chmod hint" \
    '.error.code == "usage" and (.error.hint | test("^run .chmod 600 "))'
  offline "key file missing" 2 compute kubernetes clusters kubeconfig 7 \
    --cluster-key-file "$WORK/missing.key" --output-file "$WORK/kubeconfig"
  expect_err "a missing key file is usage" \
    '.error.code == "usage" and (.error.message | test("does not exist"))'
  offline "pool size out of range" 2 compute kubernetes node-pools scale 7 3 --count 1001
  offline "kubeconfig to stdout" 2 compute kubernetes clusters kubeconfig 7 \
    --cluster-key-file "$key" --output-file -
  # The sealed kubeconfig is reached only through clusters kubeconfig, never hipc api, a
  # dry run included.
  offline "api GET of the cluster secrets --dry-run" 2 --dry-run \
    api GET /kubernetes/clusters/7/secrets
  expect_err "hipc api refuses the cluster secrets read" \
    '.error.code == "usage" and (.error.hint | test("clusters kubeconfig"))'
  # The same list as CLUSTER_SECRETS in crates/hippius-cli/tests/e2e/common.rs: every secret
  # key of the test key files, the signing and box seeds of this one and the other key's
  # signing seed.
  if grep -qF \
    -e "CwwNDg8QERITFBUWFxgZGhscHR4fICEiIyQlJicoKSo=" \
    -e "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA=" \
    -e "ZWZnaGlqa2xtbm9wcXJzdHV2d3h5ent8fX5/gIGCg4Q=" \
    -- "$OUT"/*; then
    result FAIL "no output holds a test key" "a secret key reached the output"
  else
    result PASS "no output holds a test key" ""
  fi
}

cdn_checks() {
  echo "== cdn (live, read-only) =="
  online "cdn zones list" 0 cdn zones list
  expect "zones envelope" 'has("items") and has("beta")'
  local zone read
  zone="$(field '.items[0].id')"
  for read in "cdn zones get" "cdn cap get" "cdn hostnames list" "cdn purges list" \
    "cdn rules get" "cdn stats"; do
    # shellcheck disable=SC2086 # the command's words are separate arguments
    online "$read on a missing zone" 1 $read "$MISSING_ZONE"
    expect_code "$read on a missing zone is not_found" not_found
    if [[ -n "$zone" ]]; then
      # shellcheck disable=SC2086 # the command's words are separate arguments
      online "$read" 0 $read "$zone"
    else
      result SKIP "$read" "the account has no CDN zone"
    fi
  done
  online "a missing purge" 1 cdn purges get "$MISSING_ZONE" 1
  expect_code "missing purge is not_found" not_found
  online "domain-connect on a missing zone" 1 cdn hostnames domain-connect "$MISSING_ZONE" \
    cdn.example.com
  expect_code "domain-connect on a missing zone is not_found" not_found
  waf_checks "$zone"
}

# The WAF reads. The API documents the events as for operators only and the docs say CDN
# operators and admins, so their answer on a missing zone may be a 403 before the 404.
waf_checks() {
  local zone="$1" read
  for read in "cdn waf get" "cdn waf status" "cdn waf analytics" "cdn waf rules get" \
    "cdn waf rate-limits get" "cdn waf versions list"; do
    # shellcheck disable=SC2086 # the command's words are separate arguments
    online "$read on a missing zone" 1 $read "$MISSING_ZONE"
    expect_code "$read on a missing zone is not_found" not_found
    if [[ -n "$zone" ]]; then
      # shellcheck disable=SC2086 # the command's words are separate arguments
      online "$read" 0 $read "$zone"
      # An update sends the version it read as If-Match. The docs give 0 before the first
      # publish; a null there would make every update of an unpublished zone fail.
      if [[ "$read" == "cdn waf rules get" || "$read" == "cdn waf rate-limits get" ]]; then
        expect "$read has a whole version, 0 before the first publish" \
          '(.version | type) == "number" and .version >= 0 and (.version | floor) == .version'
      fi
    else
      result SKIP "$read" "the account has no CDN zone"
    fi
  done
  online "cdn waf events on a missing zone" 1 cdn waf events "$MISSING_ZONE"
  expect_err "waf events on a missing zone is not_found or permission" \
    '.error.code == "not_found" or .error.code == "permission"'
  online "a missing WAF version" 1 cdn waf versions get "$MISSING_ZONE" 1
  expect_code "missing WAF version is not_found" not_found
  online "cdn lists list" 0 cdn lists list
  expect "lists envelope" 'has("items")'
  local list
  list="$(field '.items[0].name')"
  online "a missing WAF list" 1 cdn lists get "$MISSING_LIST"
  expect_code "missing WAF list is not_found" not_found
  if [[ -n "$list" ]]; then
    online "cdn lists get" 0 cdn lists get "$list"
    expect "list items" '.items | type == "array"'
  else
    result SKIP "cdn lists get" "the account has no WAF list"
  fi
}

# Reads only: a create, a reply, or a close would reach a person on the support team, so
# they run offline in dry_run_checks and refusal_checks.
support_checks() {
  echo "== support (live, read-only) =="
  local ticket
  online "support tickets list --limit 1" 0 support tickets list --limit 1
  expect "tickets envelope" 'has("items") and has("next_page_token")'
  ticket="$(field '.items[0].id')"
  if [[ -n "$ticket" ]]; then
    online "support tickets messages list" 0 support tickets messages list "$ticket" --limit 5
    expect "messages envelope" 'has("items") and has("next_page_token")'
  else
    skip_check "support tickets messages list" "support tickets messages list" \
      "the account has no ticket"
  fi
  online "a missing ticket's messages" 1 support tickets messages list "$MISSING_TICKET"
  expect_code "missing ticket's messages are not_found" not_found
  online "a missing ticket" 1 support tickets get "$MISSING_TICKET"
  expect_code "missing ticket is not_found" not_found
}

# The private network. Its lists are read live and must print the envelope, which no list
# the server pages (tenet 6: a paged answer here is `internal`). The rows are strict: the
# helpers check an exit code and then stdout or stderr, not one or the other, so a fresh
# account, whose lists answer that the network is not set up, fails them. Such an account
# must run `hipc network create --yes` once by hand first, which creates its network.
# `network create` and `network get` run offline only: both may create the account's
# network, so both are confirmed, `hipc schema` calls neither read_only, and `online` would
# refuse them. They, the rename, and the remove run offline, under --dry-run and then
# without --yes.
network_checks() {
  echo "== network (live reads; writes and the summary offline) =="
  local other="acct_0123456789abcdef0123" peer vpc
  online "network peers list" 0 network peers list
  expect "peers envelope" 'has("items") and .next_page_token == null'
  peer="$(field '.items[0].id')"
  online "network groups list" 0 network groups list
  expect "groups envelope" 'has("items") and .next_page_token == null'
  online "network policies list" 0 network policies list
  expect "policies envelope" 'has("items") and .next_page_token == null'
  online "network vpcs list" 0 network vpcs list
  expect "vpcs envelope" 'has("items") and .next_page_token == null'
  vpc="$(field '.items[0].id')"
  online "a missing network peer" 1 network peers get "$MISSING_KEY"
  expect_code "a missing network peer is not_found" not_found
  if [[ -n "$peer" ]]; then
    online "network peers get" 0 network peers get "$peer"
  else
    result SKIP "network peers get" "the account has no network peer"
  fi
  if [[ -n "$vpc" ]]; then
    online "network vpcs resources list" 0 network vpcs resources list "$vpc"
    online "network vpcs routers list" 0 network vpcs routers list "$vpc"
  else
    skip_check "network vpcs resources list" "network vpcs resources list" \
      "the account has no VPC network"
    skip_check "network vpcs routers list" "network vpcs routers list" \
      "the account has no VPC network"
  fi
  offline "network get --dry-run" 0 --dry-run network get
  expect_requests "network get is one GET" "GET $(route "network get" GET)"
  offline "network get without --yes" 2 network get
  expect_code "network get is confirmation_required" confirmation_required
  offline "network create --dry-run" 0 --dry-run network create
  expect_dry_run "network create is a POST of provision with no body" \
    ".method == \"POST\" and
    .url == \"$DEAD_ORIGIN$(route "network create" POST)\" and .body == null"
  offline "network create without --yes" 2 network create
  expect_code "network create is confirmation_required" confirmation_required
  offline "network peers update --dry-run" 0 --dry-run network peers update 12 --name hipc-check
  expect_dry_run "peer rename is a PATCH of the name" \
    ".method == \"PATCH\" and
    .url == \"$DEAD_ORIGIN$(route "network peers update" PATCH 12)\" and
    .body == {\"name\":\"hipc-check\"}"
  offline "network peers update without --yes" 2 network peers update 12 --name hipc-check
  expect_code "peer rename is confirmation_required" confirmation_required
  offline "network peers delete --dry-run" 0 --dry-run network peers delete 12
  expect_requests "peer remove is one DELETE" \
    "DELETE $(route "network peers delete" DELETE 12)"
  offline "network peers delete without --yes" 2 network peers delete 12
  expect_code "peer remove is confirmation_required" confirmation_required
  offline "network peer id with a slash" 2 network peers get ../12
  expect_code "a peer id with a slash is usage" usage
  offline "membership on the network sends the header" 0 --account "$other" --dry-run \
    network peers delete 12
  expect_dry_run "network takes the account header" ".account_header == \"$other\""
  offline "project on the network" 2 --project proj_0a1b2c3d4e5f60718293 network peers list
  expect_code "the network refuses a project" project_scope_unsupported
}

membership_checks() {
  echo "== membership =="
  if [[ -z "$MEMBER" ]]; then
    skip_check "membership on compute" "compute vms list" "hipc account list shows no membership"
    return
  fi
  online "membership on compute vms list" 0 --account "$MEMBER" compute vms list --debug
  stderr_has "membership vms URL" \
    "debug: GET $ORIGIN$(route "compute vms list" GET)"
  online "membership on personal ssh-keys" 0 --account "$MEMBER" api GET /ssh-keys/
  online "membership on personal notifications" 0 --account "$MEMBER" account notifications get
  online "membership on personal marketing consent" 0 --account "$MEMBER" \
    account marketing-consent get
  online "membership on personal referrals" 0 --account "$MEMBER" account referrals list
}

# Every command that writes, under --dry-run, then without the flag that gates it. Columns:
# the refusal the second run must meet (or `-` for a command with no gate, or one whose
# gate comes after a read, which needs a server), the method, the values of the route's
# placeholders in order (`-` for none, and a last word starting with `?` is the query),
# the body, and the arguments. The route itself is the one `hipc schema` lists for the
# command with that method and that many placeholders, so no path is copied here. @DIR@ is
# the directory of the input files. Rows pass 100 columns on purpose: one row per command
# keeps each one whole for grep and for the "every command" check, and a here-doc row cannot
# be continued on the next line. The ticket close is `-` because it reads the ticket before
# it asks, and offline that read fails before `confirmation_required` could be met.
dry_run_table() {
  cat <<'EOF'
confirmation_required|POST|-|{"token":"<the invite token from --token-file>"}|account invites preview --token-file @DIR@/invite
confirmation_required|POST|-|{"token":"<the invite token from --token-file>","confirm_without_email":false}|account invites accept --token-file @DIR@/invite
confirmation_required|PATCH|12|{"grants":[{"scope":"vms","level":"operator"}],"can_billing":true}|account members update 12 --grant vms=operator --billing-permission true --remove-others
confirmation_required|DELETE|12|null|account members delete 12
confirmation_required|POST|-|{"email":"a@example.com","grants":[{"scope":"vms","level":"viewer"}],"can_manage_team":false,"can_billing":false}|account invites create --email a@example.com --grant vms=viewer
confirmation_required|DELETE|34|null|account invites revoke 34
-|POST|-|{"name":"hipc-release-check","billing":null}|account projects create --name hipc-release-check
-|PATCH|proj_0a1b2c3d4e5f60718293|{"name":"hipc-release-check"}|account projects update proj_0a1b2c3d4e5f60718293 --name hipc-release-check
confirmation_required|DELETE|proj_0a1b2c3d4e5f60718293|null|account projects delete proj_0a1b2c3d4e5f60718293
-|POST|proj_0a1b2c3d4e5f60718293|{"to_account":"acct_ffffffffffffffffffff"}|account projects transfer proj_0a1b2c3d4e5f60718293 --to-account acct_ffffffffffffffffffff
confirmation_required|PUT|proj_0a1b2c3d4e5f60718293|{"monthly_limit":"100.00","thresholds":[50,90],"block_launch_over":true}|account projects budget update proj_0a1b2c3d4e5f60718293 --monthly-limit 100 --alert-at 90 --alert-at 50 --block-launches true
confirmation_required|DELETE|proj_0a1b2c3d4e5f60718293|null|account projects budget delete proj_0a1b2c3d4e5f60718293
confirmation_required|PATCH|proj_0a1b2c3d4e5f60718293|{"limits":{"vms":5,"memory_gb":null}}|account projects quota update proj_0a1b2c3d4e5f60718293 --vms 5 --memory none
-|POST|5|{"billing":"bill_9f8e7d6c5b4a39281706"}|account project-transfers accept 5 --bill-to bill_9f8e7d6c5b4a39281706
-|POST|5|null|account project-transfers decline 5
-|DELETE|5|null|account project-transfers cancel 5
-|POST|-|{"name":"hipc-release-check"}|account billing-accounts create --name hipc-release-check
-|PATCH|bill_9f8e7d6c5b4a39281706|{"name":"hipc-release-check"}|account billing-accounts update bill_9f8e7d6c5b4a39281706 --name hipc-release-check
confirmation_required|POST|-|{"amount":"50.00"}|billing top-up --amount 50 --method card
confirmation_required|POST|-|{"amount":"20.50"}|billing top-up --amount 20.5 --method bitcoin
confirmation_required|PATCH|-|{"low_credit_alerts":false,"marketing_emails":true}|account notifications update --low-credit-alerts false --marketing-emails true
-|POST|-|{"granted":false,"source":"settings"}|account marketing-consent update --granted false
-|PUT|-|{"city":"Paris","country":"FR"}|billing profile update --city Paris --country fr
-|PUT|-|{"enabled":false,"threshold":"<the setting's threshold>","target":"<the setting's target>","monthly_cap":null}|billing auto-reload update --enabled false --monthly-cap none
-|POST|-|{}|billing auto-reload card-setup
confirmation_required|POST|-|{"plan":"solo","payment":"credits"}|drive subscription subscribe --plan solo
confirmation_required|PATCH|-|{"plan":"duo","period":"annual"}|drive subscription change --plan duo --period annual
confirmation_required|PATCH|-|{"funding":"card"}|drive subscription change --pay-from card
confirmation_required|DELETE|-|null|drive subscription cancel
confirmation_required|POST|-|{"plan":"pro","rail":"credits"}|s3 subscription subscribe --plan pro
confirmation_required|PATCH|-|{"plan":"business"}|s3 subscription change --plan business
confirmation_required|PATCH|-|{"storage_cap_bytes":null}|s3 subscription change --storage-cap default
confirmation_required|DELETE|-|null|s3 subscription cancel
confirmation_required|POST|-|{"name":"hipc-dry-run-check"}|s3 buckets create hipc-dry-run-check
confirmation_required|DELETE|hipc-dry-run-check|null|s3 buckets delete hipc-dry-run-check
public_required|PUT|hipc-dry-run-check|{"policy":{"Statement":[]}}|s3 buckets acl update hipc-dry-run-check --policy-file @DIR@/policy.json
reveal_required|POST|-|{}|s3 tokens create --kind master
reveal_required|GET|hipc-dry-run-check ?key=a.txt|null|s3 buckets objects presign hipc-dry-run-check a.txt
reveal_required|POST|-|{"scope_type":"single_bucket","bucket_names":["b"]}|s3 tokens create --kind sub --bucket b --scope-type single-bucket
confirmation_required|POST|@UUID@|null|s3 tokens revoke --kind sub @UUID@
reveal_required|POST|@UUID@|null|s3 tokens rotate --kind master @UUID@
confirmation_required|POST|-|{"plan_id":1}|hub subscription subscribe --plan 1
confirmation_required|DELETE|7|null|hub subscription cancel 7
reveal_required|POST|-|{"namespace":"hipc-dry-run-check"}|hub namespace create hipc-dry-run-check
public_required|PATCH|-|{"public":true}|hub namespace update --visibility public
confirmation_required|PATCH|-|{"public":false}|hub namespace update --visibility private
reveal_required|POST|-|null|hub namespace robot rotate
reveal_required|POST|-|{"name":"hipc-dry-run-check","role":"read","expires_days":1}|hub keys create --name hipc-dry-run-check --role read --expires-in 1
reveal_required|POST|7|null|hub keys rotate 7
confirmation_required|DELETE|7|null|hub keys delete 7
confirmation_required|DELETE|hipc-check/app|null|hub repositories delete hipc-check/app
confirmation_required|DELETE|hipc-check/app v1|null|hub artifacts delete hipc-check/app v1
confirmation_required|POST|-|{"flavor":"small","region":"FR","ssh_key_ids":[1,2]}|compute vms create --flavor small --ssh-key-id 1 --ssh-key-id 2 --region FR
confirmation_required|POST|-|{"flavor":"small","ssh_public_key":"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIReleaseCheck hipc@check"}|compute vms create --flavor small --ssh-public-key-file @DIR@/id.pub
confirmation_required|POST|-|{"flavor":"small","ssh_key_ids":[1],"ssh_public_key":"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIReleaseCheck hipc@check","public_ip":true}|compute vms create --flavor small --ssh-key-id 1 --ssh-public-key-file @DIR@/id.pub --public-ip
confirmation_required|POST|-|{"flavor":"small","ssh_key_ids":[1],"os_disk_tier":"hdd","backup_interval_s":3600}|compute vms create --flavor small --ssh-key-id 1 --os-disk-tier hdd --backup-interval 3600
confirmation_required|POST|-|{"flavor":"small","ssh_key_ids":[1],"dedicated_disks":{"tier":"hdd","count":2,"min_size_gb":2000},"pinned_ack":true}|compute vms create --flavor small --ssh-key-id 1 --dedicated-disk 2000:hdd --dedicated-disk 2000:hdd
-|POST|@UUID@|null|compute vms start @UUID@
confirmation_required|POST|@UUID@|null|compute vms stop @UUID@
confirmation_required|POST|@UUID@|null|compute vms reboot @UUID@
confirmation_required|POST|@UUID@|{"flavor":"medium"}|compute vms resize @UUID@ --flavor medium
-|PUT|@UUID@|{"on_guest_poweroff":"stop"}|compute vms power-policy update @UUID@ --on-guest-poweroff stop
-|DELETE|@UUID@|null|compute vms delete @UUID@
confirmation_required|POST|@UUID@|null|compute public-ip create @UUID@
confirmation_required|DELETE|@UUID@|null|compute public-ip delete @UUID@
confirmation_required|PUT|@UUID@|{"rules":[]}|compute firewall update @UUID@ --rules-file @DIR@/rules.json --remove-others
confirmation_required|POST|@UUID@|{"port":443}|compute exposures create @UUID@ --port 443
confirmation_required|DELETE|@UUID@ 5|null|compute exposures delete @UUID@ 5
confirmation_required|PUT|@UUID@|{"interval_s":3600}|compute backups policy update @UUID@ --interval 3600
confirmation_required|DELETE|@UUID@|null|compute backups policy delete @UUID@
-|POST|@UUID@|{"point_id":"9","mode":"in_place","confirm":"<the VM's name>"}|compute backups restore @UUID@ --run-id 9
-|PUT|-|{"enabled":true}|compute runners settings update --enabled true
confirmation_required|DELETE|3|null|compute runners installations delete 3
-|POST|-|null|compute runners connect
-|POST|-|{"compose":"services:\n  web:\n    image: nginx\n"}|compute app-engine validate --compose-file @DIR@/compose.yaml
-|POST|-|{"flavor":"small","compose":"services:\n  web:\n    image: nginx\n","secrets":{"DB_PASSWORD":"<the DB_PASSWORD from --secrets-file>"},"region":"FR","backup_interval_s":3600}|compute app-engine apps create --flavor small --compose-file @DIR@/compose.yaml --secrets-file @DIR@/app-secrets.json --region FR --backup-interval 3600
-|POST|-|{"compose":"services:\n  web:\n    image: nginx\n"}|compute app-engine fix --compose-file @DIR@/compose.yaml --output-file @DIR@/fixed.yaml
-|PUT|-|{"monthly_cap":"5"}|compute runners limit update --monthly-cap 5
confirmation_required|POST|-|{"name":"hipc-dry","public_key":"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIReleaseCheck hipc@check"}|compute ssh-keys create --name hipc-dry --public-key-file @DIR@/id.pub
confirmation_required|DELETE|1|null|compute ssh-keys delete 1
reveal_required|POST|-|{"tier":"starter","size":"s","engine_version":"17"}|compute databases create --tier starter --size s --engine-version 17
confirmation_required|DELETE|56|null|compute databases delete 56
confirmation_required|POST|56|null|compute databases restart 56
reveal_required|POST|56|null|compute databases restore 56
confirmation_required|POST|56|null|compute databases exposure create 56
confirmation_required|DELETE|56|null|compute databases exposure delete 56
reveal_required|GET|7|null|compute kubernetes backups list 7
-|DELETE|7|null|compute kubernetes clusters delete 7
-|POST|7|{"name":"hipc-dry","flavor":"medium","labels":{"tier":"web"},"taints":["spot:NoSchedule"]}|compute kubernetes node-pools create 7 --name hipc-dry --flavor medium --label tier=web --taint spot:NoSchedule
-|POST|7 12|{}|compute kubernetes operations cancel 7 12
-|POST|7 12|{}|compute kubernetes operations resume 7 12
confirmation_required|POST|7 work-1|{}|compute kubernetes nodes delete 7 work-1
-|POST|-|{"gb":{"FR":100}}|cdn estimate --traffic FR=100
public_required|POST|-|{"name":"z","origin":{"kind":"s3","bucket":"b"}}|cdn zones create --name z --bucket b
confirmation_required|PATCH|@ZONE@|{"name":"z2"}|cdn zones update @ZONE@ --name z2
confirmation_required|PATCH|@ZONE@|{"cors":{"allow_origins":["https://app.example.com"]}}|cdn zones update @ZONE@ --cors-file @DIR@/cors.json
confirmation_required|DELETE|@ZONE@|null|cdn zones delete @ZONE@
-|PATCH|@ZONE@|{"monthly_cap":"5"}|cdn cap update @ZONE@ --monthly-cap 5
confirmation_required|POST|@ZONE@|{"hostname":"cdn.example.com"}|cdn hostnames create @ZONE@ cdn.example.com
-|POST|@ZONE@ cdn.example.com|null|cdn hostnames verify @ZONE@ cdn.example.com
confirmation_required|POST|@ZONE@ cdn.example.com|{"provider":"cloudflare","credentials":{"api_token":"<the api_token from --credentials-file>"}}|cdn hostnames dns-setup @ZONE@ cdn.example.com --provider cloudflare --credentials-file @DIR@/dns.json
confirmation_required|DELETE|@ZONE@ cdn.example.com|null|cdn hostnames delete @ZONE@ cdn.example.com
-|POST|@ZONE@|{"all":true}|cdn purges create @ZONE@ --everything
confirmation_required|PUT|@ZONE@|{"rules":[]}|cdn rules update @ZONE@ --rules-file @DIR@/rules.json --remove-others
-|POST|-|{"waf":{"standard":1}}|cdn estimate --waf standard=1
confirmation_required|PATCH|@ZONE@|{"mode":"on"}|cdn waf update @ZONE@ --mode on
-|POST|@ZONE@|{"request":{"path":"/"}}|cdn waf test @ZONE@ --request-file @DIR@/waf-request.json
confirmation_required|PUT|@ZONE@|{"rules":[]}|cdn waf rules update @ZONE@ --rules-file @DIR@/rules.json --remove-others
confirmation_required|PUT|@ZONE@|{"rules":[]}|cdn waf rate-limits update @ZONE@ --rules-file @DIR@/rules.json --remove-others
confirmation_required|POST|@ZONE@ 1|{}|cdn waf versions rollback @ZONE@ 1
-|POST|-|{"name":"hip_dry_run_check","kind":"ip"}|cdn lists create hip_dry_run_check --kind ip
-|PATCH|hip_dry_run_check|{"description":"d"}|cdn lists update hip_dry_run_check --description d
confirmation_required|DELETE|hip_dry_run_check|null|cdn lists delete hip_dry_run_check
confirmation_required|POST|hip_dry_run_check|{"items":["192.0.2.1"]}|cdn lists items create hip_dry_run_check --items-file @DIR@/items.txt
confirmation_required|PUT|hip_dry_run_check|{"items":["192.0.2.1"]}|cdn lists items update hip_dry_run_check --items-file @DIR@/items.txt --remove-others
confirmation_required|DELETE|hip_dry_run_check|{"items":["192.0.2.1"]}|cdn lists items delete hip_dry_run_check --items-file @DIR@/items.txt
confirmation_required|POST|-|{"subject":"hipc-release-check","description":"release check message\n","priority":"low"}|support tickets create --subject hipc-release-check --priority low --body-file @DIR@/message
-|POST|7|{"body":"release check message\n"}|support tickets reply 7 --body-file @DIR@/message
-|PATCH|7|{"status":"closed"}|support tickets update 7 --status closed
-|POST|-|{"name":"hipc-release-check"}|account referrals create --name hipc-release-check
-|PATCH|4|{"name":"hipc-release-check"}|account referrals update 4 --name hipc-release-check
EOF
}

# The writes `hipc api` holds back, in the columns of `dry_run_table` but with the path
# under the origin in place of the values: `hipc api` sends the path it is given.
api_dry_run_table() {
  cat <<'EOF'
confirmation_required|DELETE|/api/ssh-keys/1/|null|api DELETE /ssh-keys/1/
confirmation_required|DELETE|/api/cdn/lists/hipc-check/items/|{"items":[]}|api DELETE /cdn/lists/hipc-check/items/ --body-file @DIR@/items.json
confirmation_required|POST|/api/objectstore/buckets/|{"rules":[]}|api POST /objectstore/buckets/ --body-file @DIR@/rules.json
confirmation_required|GET|/api/network/me/|null|api GET /network/me/
confirmation_required|GET|/api/network/me|null|api GET /network/me
public_required|POST|/api/cdn/zones/|{"rules":[]}|api POST /cdn/zones/ --body-file @DIR@/rules.json
public_required|PATCH|/api/cdn/zones/@ZONE@/|{"origin":{"bucket":"b"}}|api PATCH /cdn/zones/@ZONE@/ --body-file @DIR@/origin.json
remove_others_required|PUT|/api/compute/vms/@UUID@/firewall/|{"rules":[]}|api PUT /compute/vms/@UUID@/firewall/ --body-file @DIR@/rules.json
remove_others_required|PUT|/api/cdn/zones/@ZONE@/rules/|{"rules":[]}|api PUT /cdn/zones/@ZONE@/rules/ --body-file @DIR@/rules.json
remove_others_required|PATCH|/api/accounts/members/12/|{"grants":[]}|api PATCH /accounts/members/12/ --body-file @DIR@/grants.json
remove_others_required|PUT|/api/cdn/zones/@ZONE@/waf/rules/|{"rules":[]}|api PUT /cdn/zones/@ZONE@/waf/rules/ --body-file @DIR@/rules.json
remove_others_required|PUT|/api/cdn/zones/@ZONE@/waf/rate-limits/|{"rules":[]}|api PUT /cdn/zones/@ZONE@/waf/rate-limits/ --body-file @DIR@/rules.json
remove_others_required|PUT|/api/cdn/lists/hipc-check/items/|{"items":[]}|api PUT /cdn/lists/hipc-check/items/ --body-file @DIR@/items.json
remove_others_required|PATCH|/api/cdn/lists/hipc-check/|{"items":[]}|api PATCH /cdn/lists/hipc-check/ --body-file @DIR@/items.json
remove_others_required|PUT|/api/v1/operator/nodes/n1/storage/|{"rules":[]}|api PUT /v1/operator/nodes/n1/storage/ --body-file @DIR@/rules.json
public_required|PUT|/api/objectstore/buckets/hipc-dry-run-check/acl/set/|{"grants":[]}|api PUT /objectstore/buckets/hipc-dry-run-check/acl/set/ --body-file @DIR@/grants.json
public_required|PATCH|/api/registry/me/publicity/|{"public":true}|api PATCH /registry/me/publicity/ --body-file @DIR@/public.json
EOF
}

# The DNS credentials file: one open to others is refused before anything is sent, and no
# refusal, failure, or --debug line prints the credential.
dns_setup_checks() {
  local dir="$1"
  cp "$dir/dns.json" "$dir/dns-open.json"
  chmod 644 "$dir/dns-open.json"
  offline "dns-setup with a file open to others" 2 cdn hostnames dns-setup "$MISSING_ZONE" \
    cdn.example.com --provider cloudflare --credentials-file "$dir/dns-open.json" --yes
  expect_err "dns-setup refuses an open file" \
    '.error.code == "usage" and (.error.hint | test("chmod 600"))'
  offline "dns-setup for the wrong provider" 2 cdn hostnames dns-setup "$MISSING_ZONE" \
    cdn.example.com --provider route53 --credentials-file "$dir/dns.json" --yes
  expect_code "dns-setup refuses another provider's fields" usage
  offline "api POST dns-setup --dry-run" 2 --dry-run api POST \
    "/cdn/zones/$MISSING_ZONE/hostnames/cdn.example.com/dns-setup/" --body-file "$dir/dns.json"
  expect_err "hipc api never sends a DNS setup" \
    '.error.code == "usage" and (.error.hint | test("hipc cdn hostnames dns-setup"))'
  offline "dns-setup --debug against a dead origin" 1 --debug cdn hostnames dns-setup \
    "$MISSING_ZONE" cdn.example.com --provider cloudflare --credentials-file "$dir/dns.json" \
    --yes
  if grep -qF -e "$FAKE_DNS_TOKEN" -- "$OUT"/*; then
    result FAIL "no output holds the DNS credential" "the credential reached the output"
  else
    result PASS "no output holds the DNS credential" ""
  fi
}

# The App Engine secrets file: one open to others, or a symlink, is refused before anything is
# sent, stdin is no form of it, `hipc api` never sends a body with `secrets`, and no dry run or
# refusal prints a value. Offline only: a launch spends, so no row sends one, and none reaches
# the launch (the redaction of its answers and errors is tested against the local server).
app_create_checks() {
  local dir="$1"
  local create=(compute app-engine apps create --flavor small --compose-file "$dir/compose.yaml")
  cp "$dir/app-secrets.json" "$dir/app-secrets-open.json"
  chmod 644 "$dir/app-secrets-open.json"
  ln -s app-secrets.json "$dir/app-secrets-linked.json"
  printf '{"compose": "x", "secrets": {"DB_PASSWORD": "%s"}}\n' "$FAKE_APP_SECRET" \
    >"$dir/app-body.json"
  offline "apps create with a secrets file open to others" 2 "${create[@]}" \
    --secrets-file "$dir/app-secrets-open.json" --yes
  expect_err "apps create refuses an open secrets file" \
    '.error.code == "usage" and (.error.hint | test("chmod 600"))'
  offline "apps create with a symlinked secrets file" 2 --dry-run "${create[@]}" \
    --secrets-file "$dir/app-secrets-linked.json"
  expect_err "apps create refuses a symlinked secrets file" \
    '.error.code == "usage" and (.error.message | test("is a symlink"))'
  offline "apps create with secrets on stdin" 2 "${create[@]}" --secrets-file - --yes
  expect_code "apps create takes no secrets on stdin" usage
  offline "apps create failover without backups" 2 "${create[@]}" --failover auto --yes
  expect_code "apps create needs backups for a failover" usage
  offline "api POST apps with secrets --dry-run" 2 --dry-run api POST \
    /compute/app-engine/apps/ --body-file "$dir/app-body.json"
  expect_err "hipc api never sends an app's secrets" \
    '.error.code == "usage" and (.error.hint | test("apps create --secrets-file"))'
  offline "api POST validate with secrets --dry-run" 2 --dry-run api POST \
    /compute/app-engine/validate/ --body-file "$dir/app-body.json"
  expect_err "hipc api never sends secrets to validate" \
    '.error.code == "usage" and (.error.hint | test("app-engine validate"))'
  if grep -qF -e "$FAKE_APP_SECRET" -- "$OUT"/*; then
    result FAIL "no output holds an app secret" "a secret value reached the output"
  else
    result PASS "no output holds an app secret" ""
  fi
}

# route_of_row <arguments> <method> <values>: sets `path` to the route `hipc schema` lists for
# the command of a `dry_run_table` row, with the row's placeholder values, and its query.
route_of_row() {
  local argv values query="" name last
  read -r -a argv <<<"${1//@DIR@/x}"
  name="$(command_path "${argv[@]}")"
  read -r -a values <<<"${3//@UUID@/$MISSING_UUID}"
  values=("${values[@]//@ZONE@/$MISSING_ZONE}")
  last=$((${#values[@]} - 1))
  if [[ "${values[*]}" == - ]]; then
    values=()
  elif [[ "${values[last]}" == \?* ]]; then
    query="${values[last]}"
    unset "values[$last]"
  fi
  path="$(route "$name" "$2" ${values[@]+"${values[@]}"} | head -n 1)$query"
}

# dry_run_row <dir> <refusal> <method> <path> <body> <arguments>: one row of a dry-run table.
# The dry run must show the method and the path, and the run without the gating flag must
# meet the refusal.
dry_run_row() {
  local dir="$1" refusal="$2" method="$3" path="$4" body="$5" line="$6" args name i
  line="${line//@UUID@/$MISSING_UUID}"
  line="${line//@ZONE@/$MISSING_ZONE}"
  path="${path//@UUID@/$MISSING_UUID}"
  path="${path//@ZONE@/$MISSING_ZONE}"
  read -r -a args <<<"$line"
  # The directory goes in after the split, so a space in its path stays in one word.
  for i in "${!args[@]}"; do
    args[i]="${args[i]//@DIR@/$dir}"
  done
  name="$(command_path "${args[@]}")"
  [[ "$name" == api ]] && name="api ${args[1]}"
  offline "dry-run $name" 0 --dry-run "${args[@]}"
  expect_dry_run "dry-run $name is $method" \
    ".dry_run == true and .method == \"$method\" and \"$path\" != \"\" and
    .url == \"$DEAD_ORIGIN$path\" and .body == $body and has(\"account_header\") and
    has(\"project_header\") and has(\"billing_header\") and
    (.requests | all(has(\"dry_run\") | not))"
  case "$refusal" in
  confirmation_required) offline "$name without --yes" 2 "${args[@]}" ;;
  reveal_required) offline "$name without --reveal" 2 "${args[@]}" --yes ;;
  public_required) offline "$name without --public" 2 "${args[@]}" --yes ;;
  remove_others_required) offline "$name without --remove-others" 2 "${args[@]}" --yes ;;
  *) return ;;
  esac
  expect_code "$name is $refusal" "$refusal"
}

dry_run_checks() {
  echo "== every write under --dry-run, then its refusal (nothing sent) =="
  local dir="$WORK/input"
  mkdir "$dir"
  printf 'invite-token-for-the-release-check\n' >"$dir/invite"
  printf '{"rules": []}\n' >"$dir/rules.json"
  printf '{"origin": {"bucket": "b"}}\n' >"$dir/origin.json"
  printf '{"grants": []}\n' >"$dir/grants.json"
  printf '{"items": []}\n' >"$dir/items.json"
  printf '{"public": true}\n' >"$dir/public.json"
  printf '{"Statement": []}\n' >"$dir/policy.json"
  printf '{"allow_origins": ["https://app.example.com"]}\n' >"$dir/cors.json"
  printf 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIReleaseCheck hipc@check\n' >"$dir/id.pub"
  printf '{"path": "/"}\n' >"$dir/waf-request.json"
  printf '192.0.2.1 # a documentation address\n' >"$dir/items.txt"
  printf 'release check message\n' >"$dir/message"
  printf 'services:\n  web:\n    image: nginx\n' >"$dir/compose.yaml"
  printf 'name: caf\xe9\n' >"$dir/latin1.yaml"
  ln -s compose.yaml "$dir/linked.yaml"
  # The DNS credential is held to the config file's checks, so it is written owner-only.
  (umask 077 && printf '{"api_token": "%s"}\n' "$FAKE_DNS_TOKEN" >"$dir/dns.json")
  (umask 077 && printf '{"DB_PASSWORD": "%s"}\n' "$FAKE_APP_SECRET" >"$dir/app-secrets.json")
  local refusal method values body line path
  while IFS='|' read -r refusal method values body line; do
    path=""
    route_of_row "$line" "$method" "$values"
    dry_run_row "$dir" "$refusal" "$method" "$path" "$body" "$line"
  done < <(dry_run_table)
  while IFS='|' read -r refusal method path body line; do
    dry_run_row "$dir" "$refusal" "$method" "$path" "$body" "$line"
  done < <(api_dry_run_table)
  dns_setup_checks "$dir"
  app_create_checks "$dir"
  # A compose goes byte for byte as a JSON string, so one that is not UTF-8 is refused before
  # anything is sent; a fix's dry run writes no file.
  offline "compose that is not UTF-8" 2 compute app-engine validate --compose-file \
    "$dir/latin1.yaml"
  expect_err "a compose that is not UTF-8 is usage" \
    '.error.code == "usage" and (.error.message | test("not UTF-8"))'
  check "fix dry run writes no file" "$dir/fixed.yaml" test ! -e "$dir/fixed.yaml"
  # A symlink is never followed, so a link to the config file cannot upload the token.
  offline "compose that is a symlink" 2 --dry-run compute app-engine validate --compose-file \
    "$dir/linked.yaml"
  expect_err "a symlinked compose is usage" \
    '.error.code == "usage" and (.error.message | test("is a symlink"))'
  offline "fixed compose to stdout" 2 compute app-engine fix --compose-file \
    "$dir/compose.yaml" --output-file -
  expect_err "a fixed compose to stdout is usage" \
    '.error.code == "usage" and (.error.message | test("stdout already holds the answer"))'
  offline "app id with a slash" 2 compute app-engine apps get vm/1
  expect_err "an app id with a slash is usage" \
    '.error.code == "usage" and (.error.message | test("a VM id is"))'
  offline "account use --dry-run" 0 --dry-run account use self
  expect "account use writes nothing" \
    '.dry_run == true and .account == "self" and (has("target") | not)'
  offline "projects use --dry-run" 0 --dry-run account projects use default
  expect "projects use writes nothing" \
    '.dry_run == true and .project == "default" and (has("target") | not)'
  offline "billing-accounts use --dry-run" 0 --dry-run account billing-accounts use default
  expect "billing-accounts use writes nothing" \
    '.dry_run == true and .billing == "default" and (has("target") | not)'
  offline "costs --dry-run" 0 --dry-run account billing-accounts costs default \
    --from 2026-09-01 --to 2026-09-30 --group-by day --group-by project
  expect_dry_run "costs query in the console's order" \
    '.url | endswith("/default/costs/?from=2026-09-01&to=2026-09-30&group=project%2Cday")'
  offline "auth login --dry-run" 0 --dry-run auth login --token-file "$dir/invite"
  expect "login dry-run stores nothing" '.dry_run == true and (has("target") | not)'
  offline "hub subscribe --replace --dry-run" 0 --dry-run hub subscription subscribe --plan 1 --replace
  expect "replace may cancel first" '.may_cancel_first.method == "DELETE"'
  expect "replace lists every request" \
    '[.requests[].method] == ["GET", "GET", "DELETE", "POST"]'
  # A plan name is looked up by a real run only, so the dry run shows a placeholder.
  offline "hub subscribe by name --dry-run" 0 --dry-run hub subscription subscribe --plan Builder
  expect_dry_run "plan name is a placeholder" '.body.plan_id == "<looked up from Builder>"'
  local subscribe="hub subscription subscribe"
  expect_requests "subscribe lists the reads" "GET $(route_ending "$subscribe" GET /plans/)" \
    "GET $(route_ending "$subscribe" GET /subscriptions/)" "POST $(route "$subscribe" POST)"
  # A bucket ACL set needs --remove-others as well as --public.
  offline "api PUT acl/set with --public only" 2 api PUT \
    /objectstore/buckets/hipc-dry-run-check/acl/set/ --body-file "$dir/grants.json" --public --yes
  expect_code "acl/set with --public only is remove_others_required" remove_others_required
  offline "api POST network/isolation without --change-isolation" 2 api POST \
    /network/isolation/ --yes
  expect_code "isolation without the flag is change_isolation_required" \
    change_isolation_required
  offline "api POST network/provision without --yes" 2 api POST /network/provision/
  expect_code "api network provision is confirmation_required" confirmation_required
  offline "acl update with --public only" 2 s3 buckets acl update hipc-dry-run-check \
    --policy-file "$dir/policy.json" --public --yes
  expect_code "acl update with --public only is remove_others_required" remove_others_required
  # The token check, then the signed-in user.
  offline "auth status --dry-run lists both reads" 0 --dry-run auth status
  expect_requests "status reads the user after the token" \
    "GET $(route_ending "auth status" GET /accounts/)" "GET $(route_ending "auth status" GET /me/)"
  # A restore reads the VM, and an accept previews the invite, before the write.
  offline "restore --dry-run lists the read" 0 --dry-run compute backups restore \
    "$MISSING_UUID" --run-id 9
  expect_requests "restore reads the VM first" \
    "GET $(route "compute backups restore" GET "$MISSING_UUID")" \
    "POST $(route "compute backups restore" POST "$MISSING_UUID")"
  # Whether a cap change is confirmed depends on the cap in force, so it reads that first.
  offline "runner limit update --dry-run lists the read" 0 --dry-run compute runners limit \
    update --monthly-cap none
  expect_requests "runner limit update reads the limit first" \
    "GET $(route "compute runners limit update" GET)" \
    "PUT $(route "compute runners limit update" PUT)"
  expect_dry_run "runner limit removal sends null" '.body == {"monthly_cap": null}'
  # So do a CDN cap update and a runner settings update, and a VM delete reads the VM it names.
  offline "cdn cap update --dry-run lists the read" 0 --dry-run cdn cap update \
    "$MISSING_ZONE" --monthly-cap 5
  expect_requests "cdn cap update reads the cap first" \
    "GET $(route "cdn cap update" GET "$MISSING_ZONE")" \
    "PATCH $(route "cdn cap update" PATCH "$MISSING_ZONE")"
  offline "runner settings update --dry-run lists the read" 0 --dry-run compute runners \
    settings update --enabled false
  expect_requests "runner settings update reads the settings first" \
    "GET $(route "compute runners settings update" GET)" \
    "PUT $(route "compute runners settings update" PUT)"
  offline "vms delete --dry-run lists the read" 0 --dry-run compute vms delete "$MISSING_UUID"
  expect_requests "vms delete reads the VM first" \
    "GET $(route "compute vms delete" GET "$MISSING_UUID")" \
    "DELETE $(route "compute vms delete" DELETE "$MISSING_UUID")"
  offline "accept --dry-run lists the preview" 0 --dry-run account invites accept \
    --token-file "$dir/invite"
  expect_requests "accept previews first" \
    "POST $(route_ending "account invites accept" POST /preview/)" \
    "POST $(route_ending "account invites accept" POST /accept/)"
  # Without --remove-others a list replace reads the current list before its question, so
  # its dry run names that read; the table rows above pass the flag to reach the question.
  local replace
  for replace in "compute firewall update $MISSING_UUID --rules-file @DIR@/rules.json" \
    "cdn rules update $MISSING_ZONE --rules-file @DIR@/rules.json" \
    "cdn waf rules update $MISSING_ZONE --rules-file @DIR@/rules.json" \
    "cdn waf rate-limits update $MISSING_ZONE --rules-file @DIR@/rules.json" \
    "cdn lists items update $MISSING_LIST --items-file @DIR@/items.txt" \
    "account members update 12 --grant vms=operator"; do
    read -r -a args <<<"$replace"
    # The directory goes in after the split, as in the table above.
    for i in "${!args[@]}"; do
      args[i]="${args[i]//@DIR@/$dir}"
    done
    name="$(command_path "${args[@]}")"
    offline "$name --dry-run" 0 --dry-run "${args[@]}"
    expect "$name reads first" '.reads_first.method == "GET"'
    expect_dry_run "$name lists the read, then the write" \
      '[.requests[].method] == ["GET", .method] and .requests[0] == .reads_first'
  done
}

refusal_checks() {
  echo "== refusals (nothing sent) =="
  # Any well-formed account id selects a membership; the refusal is local.
  local other="acct_0123456789abcdef0123" key="$WORK/input/id"
  offline "membership on drive" 2 --account "$other" drive subscription get
  expect_code "drive refuses a membership" account_scope_unsupported
  offline "membership on hub" 2 --account "$other" hub subscription get
  expect_code "hub refuses a membership" account_scope_unsupported
  offline "membership on hub usage" 2 --account "$other" hub usage
  expect_code "hub usage refuses a membership" account_scope_unsupported
  offline "membership on hub keys" 2 --account "$other" hub keys list
  expect_code "hub keys refuse a membership" account_scope_unsupported
  # --reveal is not a confirmation: a create with it still needs --yes, and a rotate with
  # --yes still needs --reveal. A role that is not a lowercase code is usage; which roles exist
  # is the server's to say.
  offline "hub keys create without --yes" 2 hub keys create --name hipc-dry-run-check \
    --role read --reveal
  expect_code "hub keys create is confirmation_required" confirmation_required
  offline "hub keys rotate without --yes" 2 hub keys rotate 7 --reveal
  expect_code "hub keys rotate is confirmation_required" confirmation_required
  offline "hub robot rotate without --yes" 2 hub namespace robot rotate --reveal
  expect_code "hub robot rotate is confirmation_required" confirmation_required
  offline "hub namespace public without --yes" 2 hub namespace update --visibility public \
    --public
  expect_code "hub namespace public is confirmation_required" confirmation_required
  offline "membership on hub models" 2 --account "$other" hub models list
  expect_code "hub models refuse a membership" account_scope_unsupported
  offline "membership on hub events" 2 --account "$other" hub events list
  expect_code "hub events refuse a membership" account_scope_unsupported
  offline "model without its namespace" 2 hub models get llama
  expect_code "a model needs its namespace" usage
  offline "hub keys create with a role that is no code" 2 hub keys create \
    --name hipc-dry-run-check --role Read --reveal --yes
  expect_code "a role that is no code is usage" usage
  offline "membership on hub repositories" 2 --account "$other" hub repositories list
  expect_code "hub repositories refuse a membership" account_scope_unsupported
  offline "membership on a hub delete --dry-run" 2 --account "$other" --dry-run \
    hub artifacts delete hipc-check/app v1
  expect_code "a hub delete dry run refuses a membership" account_scope_unsupported
  offline "repository without its namespace" 2 hub artifacts list app
  offline "repository with an artifacts part" 2 hub artifacts list hipc-check/artifacts/app
  offline "artifact reference with a slash" 2 hub artifacts get hipc-check/app v1/x
  offline "membership on billing transactions" 2 --account "$other" billing transactions
  expect_code "transactions refuse a membership" account_scope_unsupported
  offline "membership on drive history" 2 --account "$other" drive subscription history
  expect_code "drive history refuses a membership" account_scope_unsupported
  offline "membership on invoices sends the header" 0 --account "$other" --dry-run \
    billing invoices list
  expect_dry_run "invoices account header" ".account_header == \"$other\""
  expect "one request is the one entry of requests" \
    '(.requests | length) == 1 and (.requests[0] | has("dry_run") | not) and
    (keys == ["dry_run", "requests"])'
  offline "usage month outside YYYY-MM" 2 billing usage --month 2026-13
  expect_code "a bad month is usage" usage
  offline "invoice to stdout" 2 billing invoices get 1 --output-file -
  expect_code "stdout is not an invoice file" usage
  # A top-up starts a payment, so every row stays offline.
  offline "top-up below the card minimum" 2 billing top-up --amount 9.99 --method card --yes
  expect_code "a card top-up under the range is usage" usage
  offline "top-up past the Bitcoin maximum" 2 \
    billing top-up --amount 100000.01 --method bitcoin --yes
  expect_code "a Bitcoin top-up over the range is usage" usage
  offline "top-up amount past the cent" 2 billing top-up --amount 10.001 --method card --yes
  expect_code "an amount past the cent is usage" usage
  offline "membership on top-up" 2 --account "$other" billing top-up --amount 50 --method card \
    --yes
  expect_code "top-up refuses a membership" account_scope_unsupported
  offline "membership on top-up --dry-run" 2 --account "$other" --dry-run \
    billing top-up --amount 50 --method bitcoin
  expect_code "a top-up dry run refuses a membership" account_scope_unsupported
  offline "top-up --dry-run lists its request" 0 --dry-run \
    billing top-up --amount 50 --method card
  expect_requests "top-up lists one POST" \
    "POST $(route_ending "billing top-up" POST /create-checkout-session/)"
  offline "hAlpha top-up --dry-run lists its reads, the buy, and the follow" 0 --dry-run \
    billing top-up --method halpha --alpha 1.5
  local purchases
  purchases="GET $(route_ending "billing top-up" GET /purchases/)?ordering=-created_at"
  expect_requests "hAlpha top-up lists the quote, the list, the buy, and the follow" \
    "GET $(route_ending "billing top-up" GET /quote/)" "$purchases" \
    "POST $(route_ending "billing top-up" POST /buy/)" "$purchases"
  expect "the buy sends the exact amount" '.body.alpha_amount == "1500000000000000000"'
  offline "hAlpha below the chain minimum" 2 billing top-up --method halpha --alpha 0.009 --yes
  expect_code "a buy under 0.01 hAlpha is usage" usage
  offline "hAlpha past 18 decimals" 2 billing top-up --method halpha \
    --alpha 1.0000000000000000001 --yes
  expect_code "a 19th decimal is usage" usage
  offline "dollars on a hAlpha top-up" 2 billing top-up --method halpha --amount 50 --yes
  expect_code "--amount with halpha is usage" usage
  offline "hAlpha on a card top-up" 2 billing top-up --method card --alpha 1 --yes
  expect_code "--alpha with card is usage" usage
  # The access-key login is gone: its flag is a plain usage error, and nothing is sent.
  offline "login --access-key is gone" 2 auth login --access-key
  expect_code "the removed access-key flag is usage" usage
  offline "login --access-key-file is gone" 2 auth login --access-key-file "$WORK/key.txt"
  expect_code "the removed access-key file flag is usage" usage
  offline "login --new-account is gone" 2 auth login --new-account
  expect_code "the removed new-account flag is usage" usage
  # The profile PUT keeps the fields it leaves out, so the update sends only those given.
  offline "profile update --dry-run is one PUT" 0 --dry-run billing profile update \
    --tax-id GB1
  expect_requests "profile update reads nothing first" "PUT $(route "billing profile update" PUT)"
  expect_dry_run "profile update sends only the field given" '.body == {"tax_id": "GB1"}'
  # A new billing email is confirmed unless the profile holds it, so that update reads first.
  offline "profile email update --dry-run lists the read" 0 --dry-run billing profile update \
    --billing-email invoices@example.com
  expect_requests "an email update reads the profile first" \
    "GET $(route "billing profile update" GET)" "PUT $(route "billing profile update" PUT)"
  # The auto-reload PUT takes the whole setting, so the update reads it first.
  offline "auto-reload update --dry-run lists the read" 0 --dry-run billing auto-reload \
    update --threshold 10
  expect_requests "auto-reload update reads the setting first" \
    "GET $(route "billing auto-reload update" GET)" "PUT $(route "billing auto-reload update" PUT)"
  # Whether it asks depends on the setting in force, so a run without --yes reads first: the
  # dead origin refuses that read, and nothing is written.
  offline "auto-reload update without --yes reads first" 1 billing auto-reload update \
    --enabled true
  expect_code "auto-reload update fails on the read" unavailable
  offline "auto-reload amount past the cent" 2 billing auto-reload update --target 10.001
  expect_code "an auto-reload amount past the cent is usage" usage
  offline "auto-reload update with nothing to change" 2 billing auto-reload update
  expect_code "an empty auto-reload update is usage" usage
  offline "profile country that is not a code" 2 billing profile update --country FRA
  expect_code "a country that is not two letters is usage" usage
  offline "notifications update with nothing to change" 2 account notifications update
  expect_code "an empty notifications update is usage" usage
  offline "marketing consent that is not a boolean" 2 account marketing-consent update \
    --granted yes
  expect_code "a consent that is not a boolean is usage" usage
  offline "marketing consent given from hipc" 2 account marketing-consent update --granted true
  expect_code "giving the consent from hipc is usage" usage
  offline "membership on s3 sends the header" 0 --account "$other" --dry-run s3 buckets list
  expect_dry_run "account header" ".account_header == \"$other\""
  selector_checks "$other"
  offline "hub has no --pay-from" 2 hub subscription subscribe --plan 1 --pay-from credits --yes
  expect_code "hub --pay-from is usage" usage
  printf -- '-----BEGIN OPENSSH PRIVATE KEY-----\nzzzz\n-----END OPENSSH PRIVATE KEY-----\n' >"$key"
  offline "ssh-keys create with a private key" 2 \
    compute ssh-keys create --name k --public-key-file "$key" --yes
  expect_err "private key refused" '.error.code == "usage" and (.error.message | test("private key"))'
  offline "vms create with a private key" 2 \
    compute vms create --flavor small --ssh-public-key-file "$key" --yes
  expect_err "vms create refuses a private key" \
    '.error.code == "usage" and (.error.message | test("private key"))'
  offline "bad bucket name" 2 s3 buckets delete Bad_Name --yes
  offline "bad namespace" 2 hub namespace availability Bad_Name
  offline "key id with a slash" 2 s3 tokens revoke --kind sub ../x --yes
  offline "hub subscription id not a number" 2 hub subscription cancel x --yes
  offline "vm id with a slash" 2 compute vms get ../ssh-keys
  offline "failovers vm id with a slash" 2 compute vms failovers ../ssh-keys
  offline "power policy that is not restart or stop" 2 compute vms power-policy update \
    "$MISSING_UUID" --on-guest-poweroff shutdown
  expect_code "an unknown power policy is usage" usage
  offline "ssh key id not a number" 2 compute ssh-keys get x
  offline "database metrics id not a number" 2 compute databases metrics x
  offline "bad bucket name for acl" 2 s3 buckets acl get Bad_Name
  offline "feasibility region not two letters" 2 compute feasibility --flavor small --region FRA
  expect_code "a three-letter region is usage" usage
  offline "dot segment in hipc api" 2 api GET /ssh-keys/../compute/vms/
  offline "vms create without a key" 2 compute vms create --flavor small --yes
  offline "--no-wait with --wait-timeout" 2 \
    compute vms create --flavor small --ssh-key-id 1 --no-wait --wait-timeout 5 --yes
  offline "--wait with --no-wait" 2 \
    compute vms create --flavor small --ssh-key-id 1 --wait --no-wait --yes
  # --wait-timeout asks for a wait even on a pipe, where the launch does not wait by
  # default, so it is no longer usage there; a dry run takes it and sends nothing.
  offline "--wait-timeout on a pipe --dry-run" 0 --dry-run \
    compute vms create --flavor small --ssh-key-id 1 --wait-timeout 5
  expect_dry_run "--wait-timeout on a pipe is not usage" '.dry_run == true and .method == "POST"'
  offline "bad placement group" 2 \
    compute vms create --flavor small --ssh-key-id 1 --placement-group Bad_Group --yes
  # The launch options, each checked before anything is sent.
  local vm=(compute vms create --flavor small --ssh-key-id 1)
  offline "os disk tier outside the enum" 2 "${vm[@]}" --os-disk-tier sata --yes
  offline "launch backup interval of no seconds" 2 "${vm[@]}" --backup-interval 0 --yes
  offline "dedicated disk under 1000 GiB" 2 "${vm[@]}" --dedicated-disk 999:hdd --yes
  expect_err "a small dedicated disk is usage" \
    '.error.code == "usage" and (.error.message | test("a dedicated disk is"))'
  offline "dedicated disks that differ" 2 "${vm[@]}" --dedicated-disk 1000:hdd \
    --dedicated-disk 1000:ssd --yes
  offline "dedicated disk with backups" 2 "${vm[@]}" --dedicated-disk 1000:hdd \
    --backup-interval 3600 --yes
  offline "feasibility tier outside the enum" 2 compute feasibility --flavor small \
    --os-disk-tier sata
  offline "backup interval outside the set" 2 \
    compute backups policy update "$MISSING_UUID" --interval 600 --yes
  # The local checks: each refuses before anything is read or sent.
  local typo="$WORK/input/fw-typo.json" cache="$WORK/input/cache-typo.json" keys=() i
  printf '{"rules": [{"protocol": "tcp", "ports": ["22"], "sources": ["0.0.0.0/0"]}]}\n' >"$typo"
  offline "firewall rule key the API does not list" 2 \
    compute firewall update "$MISSING_UUID" --rules-file "$typo" --yes
  expect_err "a misspelled firewall key is usage" \
    '.error.code == "usage" and (.error.message | test("the key .sources."))'
  printf '[{"match": {"glob": "*"}, "actions": {"ttl": 60}}]\n' >"$cache"
  offline "cache rule key the API does not list" 2 \
    cdn rules update "$MISSING_ZONE" --rules-file "$cache" --yes
  expect_err "a misspelled cache action is usage" \
    '.error.code == "usage" and (.error.message | test("the key .ttl."))'
  local waf_typo="$WORK/input/waf-typo.json" waf_request="$WORK/input/waf-typo-request.json"
  printf '{"rules": [{"expr": "true", "action": "block"}]}\n' >"$waf_typo"
  offline "WAF rule key the API does not list" 2 \
    cdn waf rules update "$MISSING_ZONE" --rules-file "$waf_typo" --yes
  expect_err "a misspelled WAF rule key is usage" \
    '.error.code == "usage" and (.error.message | test("the key .expr."))'
  printf '{"url": "/"}\n' >"$waf_request"
  offline "WAF test request key the docs do not list" 2 \
    cdn waf test "$MISSING_ZONE" --request-file "$waf_request"
  expect_code "a WAF test request key is usage" usage
  offline "WAF list name with a capital" 2 cdn lists get Bad_Name
  offline "estimate past 10^6 WAF zones" 2 cdn estimate --waf standard=1000001
  for i in $(seq 21); do keys+=(--ssh-key-id "$i"); done
  offline "vms create with 21 key ids" 2 compute vms create --flavor small "${keys[@]}" --yes
  expect_code "more than 20 key ids is usage" usage
  offline "purge path without a slash" 2 cdn purges create "$MISSING_ZONE" --path a.jpg
  offline "zone prefix without a trailing slash" 2 \
    cdn zones create --name site --bucket web --prefix site --public --yes
  offline "estimate past 10^9 GB" 2 cdn estimate --traffic FR=1000000001
  offline "history limit past 200" 2 s3 subscription history --limit 201
  # --credentials-dir is checked before anything is sent; a dry run with a good
  # directory sends nothing and writes no file.
  local creds="$WORK/input/creds" db=(compute databases create --tier starter --size s
    --engine-version 17)
  offline "credentials dir that does not exist" 2 "${db[@]}" \
    --credentials-dir "$WORK/input/no-such-dir" --reveal --yes
  expect_err "a missing credentials dir is usage" \
    '.error.code == "usage" and (.error.message | test("does not exist"))'
  mkdir "$creds"
  offline "credentials dir --dry-run" 0 --dry-run "${db[@]}" --credentials-dir "$creds"
  expect_dry_run "credentials dir leaves the body alone" \
    '.method == "POST" and .body == {"tier":"starter","size":"s","engine_version":"17"}'
  check "credentials dry run writes no file" "$creds" test ! -e "$creds/client.key"
  : >"$creds/client.key"
  offline "credentials dir holding client.key" 2 "${db[@]}" --credentials-dir "$creds" \
    --reveal --yes
  expect_err "an existing client.key is usage" \
    '.error.code == "usage" and (.error.message | test("client.key"))'
  offline "database id not a number" 2 compute databases get x
  expect_err "a numeric id names its list" \
    '.error.message | test("from .hipc compute databases list.")'
  offline "transactions page token" 2 billing transactions --page-token x
  offline "runner jobs page token" 2 compute runners jobs --page-token abc
  # A blank message is refused before the question, even with --yes.
  local blank="$WORK/input/blank"
  printf ' \n' >"$blank"
  offline "ticket create with a blank message" 2 \
    support tickets create --subject hipc-release-check --body-file "$blank" --yes
  expect_code "a blank ticket message is usage" usage
  offline "ticket reply with a blank message" 2 support tickets reply 7 --body-file "$blank"
  expect_code "a blank reply is usage" usage
  # A message that holds the API token is refused before anything is sent, a dry run too.
  local leak="$WORK/input/leak"
  printf 'my token is %s\n' "$FAKE_TOKEN" >"$leak"
  offline "ticket reply holding the token" 2 --dry-run support tickets reply 7 --body-file "$leak"
  expect_code "a reply holding the token is usage" usage
  offline "ticket id not a number" 2 support tickets get x
  # The ticket routes act as the signed-in user, so a membership sends no header.
  offline "membership on support sends no header" 0 --account "$other" --dry-run \
    support tickets list
  expect_dry_run "support has no account header" '.account_header == null'
  offline "membership on a ticket close sends no header" 0 --account "$other" --dry-run \
    support tickets update 7 --status closed
  expect_dry_run "a close has no account header" '.account_header == null'
  # The close reads the ticket first, so its question names the subject.
  expect_requests "a close lists the read, then the PATCH" \
    "GET $(route "support tickets update" GET 7)" "PATCH $(route "support tickets update" PATCH 7)"
  offline "membership on a referral create sends no header" 0 --account "$other" --dry-run \
    account referrals create --name hipc-release-check
  expect_dry_run "a referral create has no account header" '.account_header == null'
  offline "ticket create --dry-run lists its request" 0 --dry-run \
    support tickets create --subject hipc-release-check --body-file "$WORK/input/message"
  expect_requests "ticket create lists the POST" "POST $(route "support tickets create" POST)"
  offline "ticket reply --dry-run lists its request" 0 --dry-run \
    support tickets reply 7 --body-file "$WORK/input/message"
  expect_requests "ticket reply lists the POST" "POST $(route "support tickets reply" POST 7)"
  # A whole-ticket update (`PUT`), which the customer console does not send, is not personal:
  # `hipc api` refuses it with a membership. The customer's close (`PATCH`) is, so it goes
  # out without the header.
  offline "membership on a ticket PUT" 2 --account "$other" \
    api PUT /support/tickets/7/ --body-file "$WORK/input/rules.json" --yes
  expect_code "a ticket PUT refuses a membership" account_scope_unsupported
  # `hipc api` sends that PATCH with the body given: the server is the guard on which
  # fields of a ticket a customer may change.
  offline "membership on a ticket PATCH through hipc api" 0 --account "$other" --dry-run \
    api PATCH /support/tickets/7/ --body-file "$WORK/input/rules.json"
  expect_dry_run "a ticket PATCH through hipc api keeps its body and has no account header" \
    '.method == "PATCH" and .account_header == null and .body == {"rules":[]}'
  # A host that never resolves: a regression would fail to connect, and the token is fake.
  capture "plain http origin refused" 2 env HIPPIUS_TOKEN="$FAKE_TOKEN" \
    "$HIPC" --api-url http://hipc-release-check.invalid auth status
  expect_code "plain http is usage" usage
  # HIPPIUS_TOKEN belongs to the default origin unless HIPPIUS_TOKEN_ORIGIN names another,
  # so --api-url alone cannot send it elsewhere; nothing is sent.
  capture "HIPPIUS_TOKEN withheld from another origin" 1 env HIPPIUS_TOKEN="$FAKE_TOKEN" \
    "$HIPC" --api-url "$DEAD_ORIGIN" account list
  expect_code "a withheld HIPPIUS_TOKEN is auth" auth
  # hipc api sends only to the API origin, and never to the account token route.
  offline "hipc api refuses another origin" 2 --dry-run api GET https://hipc-release-check.invalid/x/
  expect_code "another origin is usage" usage
  offline "hipc api refuses the account token route" 2 --dry-run api GET /auth/token/ --reveal
  expect_code "the account token route is usage" usage
}

s3_configure_checks() {
  echo "== s3 configure and logout (local writes under a temporary HOME) =="
  local secret="$WORK/input/secret"
  printf 'fake-secret-for-the-release-check\n' >"$secret"
  local_write "s3 configure" 0 s3 configure --access-key-id hip_RELEASECHECK \
    --secret-key-file "$secret"
  expect "configure wrote" '.secret_written == true and .aws_profile == "hippius"'
  local mode
  mode="$(mode_of "$HOME/.aws/credentials")"
  check "aws credentials mode" "$mode" test "$mode" = 600
  check "aws config endpoint" "" grep -q 'endpoint_url = https://s3.hippius.com' "$HOME/.aws/config"
  local_write "configure again without --yes" 2 s3 configure --access-key-id hip_RELEASECHECK \
    --secret-key-file "$secret"
  expect_code "overwrite needs confirmation" confirmation_required
  # The document `s3 tokens create --reveal` prints, with a fake secret, in a file.
  local created="$WORK/input/created.json"
  printf '{"kind":"master","access_key_id":"hip_RELEASECHECK","secret":"fake-secret"}\n' \
    >"$created"
  local_write "configure from a token document" 0 s3 configure --aws-profile hipc-release-doc \
    --from-token-file "$created"
  expect "configure from the document wrote" '.secret_written == true'
  # A literal `$` the AWS CLI would expand. The path is under the work directory, so a
  # regression that writes it as written stays there.
  local literal="$WORK/\$AWS_DIR/config"
  capture "configure refuses a path the AWS CLI expands" 2 env AWS_CONFIG_FILE="$literal" \
    "$HIPC" s3 configure --access-key-id hip_RELEASECHECK --secret-key-file "$secret" --yes
  expect_code "an unexpanded AWS path is usage" usage
  check "nothing at the unexpanded path" "" test ! -e "$literal"
  local_write "logout without --yes" 2 auth logout
  expect_code "logout needs confirmation" confirmation_required
  local_write "logout --yes" 0 auth logout --yes
  expect "logged out" '.token_stored == false'
}

# The project and billing account selectors: sent where the route takes them,
# refused with nothing sent where it does not. Every row is offline.
selector_checks() {
  local other="$1" project="proj_0a1b2c3d4e5f60718293" billing="bill_9f8e7d6c5b4a39281706"
  offline "project on vms sends the header" 0 --project "$project" --dry-run compute vms list
  expect_dry_run "project header" ".project_header == \"$project\""
  offline "project on the S3 plan read" 0 --project "$project" --dry-run s3 plans
  expect_dry_run "S3 plan read takes a project" ".project_header == \"$project\""
  offline "billing on the balance sends the header" 0 --billing "$billing" --dry-run \
    billing balance
  expect_dry_run "billing header" ".billing_header == \"$billing\""
  offline "billing on the profile update" 0 --billing "$billing" --dry-run billing profile \
    update --city Paris
  expect_dry_run "profile update takes a billing account" \
    ".billing_header == \"$billing\" and (.requests | all(.billing_header == \"$billing\"))"
  offline "billing on the auto-reload update" 0 --billing "$billing" --dry-run billing \
    auto-reload update --enabled false
  expect_dry_run "auto-reload update takes a billing account" \
    ".billing_header == \"$billing\" and (.requests | all(.billing_header == \"$billing\"))"
  offline "billing on the card setup" 0 --billing "$billing" --dry-run billing auto-reload \
    card-setup
  expect_dry_run "card setup takes a billing account" ".billing_header == \"$billing\""
  offline "project on the profile" 2 --project "$project" billing profile get
  expect_code "the profile refuses a project" project_scope_unsupported
  # A member names a billing account on these routes, as for a top-up: without one the
  # account header alone could reach the caller's own profile or wallet.
  offline "membership on the profile update" 2 --account "$other" --dry-run billing profile \
    update --city Paris
  expect_code "profile update refuses a membership" account_scope_unsupported
  offline "membership on the auto-reload update" 2 --account "$other" --dry-run billing \
    auto-reload update --enabled false
  expect_code "auto-reload update refuses a membership" account_scope_unsupported
  offline "membership on the card setup" 2 --account "$other" billing auto-reload card-setup
  expect_code "card setup refuses a membership" account_scope_unsupported
  offline "membership on the auto-reload read" 2 --account "$other" billing auto-reload get
  expect_code "auto-reload read refuses a membership" account_scope_unsupported
  offline "membership on the auto-reload history" 2 --account "$other" --dry-run billing \
    auto-reload history
  expect_code "auto-reload history refuses a membership" account_scope_unsupported
  offline "membership with a billing account on the profile" 0 --account "$other" \
    --billing "$billing" --dry-run billing profile get
  expect_dry_run "profile takes both headers" \
    ".account_header == \"$other\" and .billing_header == \"$billing\""
  offline "membership with a billing account on the auto-reload read" 0 --account "$other" \
    --billing "$billing" --dry-run billing auto-reload get
  expect_dry_run "auto-reload read takes both headers" \
    ".account_header == \"$other\" and .billing_header == \"$billing\""
  offline "project on the runner limit" 0 --project "$project" --dry-run compute runners \
    limit update --monthly-cap 5
  expect_dry_run "runner limit takes a project" \
    ".project_header == \"$project\" and (.requests | all(.project_header == \"$project\"))"
  offline "project on a personal route" 0 --project "$project" --dry-run compute ssh-keys list
  expect_dry_run "personal route sends no project" '.project_header == null'
  offline "project on drive" 2 --project "$project" drive subscription get
  expect_code "drive refuses a project" project_scope_unsupported
  offline "project on an S3 plan write" 2 --project "$project" --dry-run s3 subscription cancel
  expect_code "an S3 plan write refuses a project" project_scope_unsupported
  offline "project on the projects list" 2 --project "$project" account projects list
  expect_code "the accounts API refuses a project" project_scope_unsupported
  offline "project on a project create" 2 --project "$project" --dry-run account projects \
    create --name hipc-release-check
  expect_code "a project create refuses a project" project_scope_unsupported
  # A stored project is left off a write to a route that names the project in its path,
  # and still refuses a create, which names none (tenet 12). Its own config file keeps the
  # selection out of every other check.
  local stored="$WORK/stored-project.toml"
  capture "store a project" 0 env HIPPIUS_CONFIG="$stored" "$HIPC" account projects use \
    "$project"
  capture "stored project on a budget update" 0 env HIPPIUS_CONFIG="$stored" \
    HIPPIUS_TOKEN="$FAKE_TOKEN" HIPPIUS_TOKEN_ORIGIN="$DEAD_ORIGIN" "$HIPC" --api-url "$DEAD_ORIGIN" --dry-run account projects \
    budget update "$project" --monthly-limit 1 --alert-at 50 --block-launches true --yes
  expect_dry_run "a budget update skips the stored project" \
    '.project_header == null and (.requests | all(.project_header == null))'
  capture "stored project on a quota update" 0 env HIPPIUS_CONFIG="$stored" \
    HIPPIUS_TOKEN="$FAKE_TOKEN" HIPPIUS_TOKEN_ORIGIN="$DEAD_ORIGIN" "$HIPC" --api-url "$DEAD_ORIGIN" --dry-run account projects \
    quota update "$project" --vms 5 --yes
  expect_dry_run "a quota update skips the stored project" '.project_header == null'
  capture "stored project on a project create" 2 env HIPPIUS_CONFIG="$stored" \
    HIPPIUS_TOKEN="$FAKE_TOKEN" HIPPIUS_TOKEN_ORIGIN="$DEAD_ORIGIN" "$HIPC" --api-url "$DEAD_ORIGIN" --dry-run account projects \
    create --name hipc-release-check
  expect_code "a project create refuses the stored project" project_scope_unsupported
  # A stored billing account is left off the rename, whose path names the billing account,
  # and off a create, which takes none; `--billing` still refuses the rename (tenet 12).
  local stored_billing="$WORK/stored-billing.toml"
  capture "store a billing account" 0 env HIPPIUS_CONFIG="$stored_billing" "$HIPC" account \
    billing-accounts use "$billing"
  capture "stored billing account on its rename" 0 env HIPPIUS_CONFIG="$stored_billing" \
    HIPPIUS_TOKEN="$FAKE_TOKEN" HIPPIUS_TOKEN_ORIGIN="$DEAD_ORIGIN" "$HIPC" --api-url "$DEAD_ORIGIN" --dry-run account \
    billing-accounts update "$billing" --name hipc-release-check
  expect_dry_run "a rename skips the stored billing account" '.billing_header == null'
  capture "stored billing account on a create" 0 env HIPPIUS_CONFIG="$stored_billing" \
    HIPPIUS_TOKEN="$FAKE_TOKEN" HIPPIUS_TOKEN_ORIGIN="$DEAD_ORIGIN" "$HIPC" --api-url "$DEAD_ORIGIN" --dry-run account \
    billing-accounts create --name hipc-release-check
  expect_dry_run "a create skips the stored billing account" '.billing_header == null'
  offline "billing on a billing account rename" 2 --billing "$billing" --dry-run account \
    billing-accounts update "$billing" --name hipc-release-check
  expect_code "a rename refuses a billing account" billing_scope_unsupported
  offline "billing on a budget update" 2 --billing "$billing" account projects budget update \
    "$project" --monthly-limit 1 --alert-at 50 --block-launches true --yes
  expect_code "a budget update refuses a billing account" billing_scope_unsupported
  offline "membership on a project rename" 0 --account "$other" --dry-run account projects \
    update "$project" --name hipc-release-check
  expect_dry_run "a project rename takes the account header" ".account_header == \"$other\""
  offline "membership on the transfers" 0 --account "$other" --project "$project" --dry-run \
    account project-transfers accept 5
  expect_dry_run "the transfers are personal" '.account_header == null and .project_header == null'
  offline "billing on vms" 2 --billing "$billing" compute vms list
  expect_code "vms refuse a billing account" billing_scope_unsupported
  offline "project on app engine" 0 --account "$other" --project "$project" --dry-run \
    compute app-engine apps list
  expect_dry_run "app engine takes both headers" \
    ".account_header == \"$other\" and .project_header == \"$project\""
  offline "billing on app engine" 2 --billing "$billing" compute app-engine get
  expect_code "app engine refuses a billing account" billing_scope_unsupported
  offline "a malformed project" 2 --project proj_x compute vms list
  expect_code "a malformed project is usage" usage
  offline "membership top-up with a billing account" 0 --account "$other" --billing "$billing" \
    --dry-run billing top-up --amount 50 --method card
  expect_dry_run "both headers" ".account_header == \"$other\" and .billing_header == \"$billing\""
  offline "a grant on one project" 0 --dry-run account invites create --email a@example.com \
    --grant "vms=viewer@$project"
  expect_dry_run "grant names the project" ".body.grants[0].project == \"$project\""
}

final_checks() {
  echo "== every command, and the output =="
  local missing
  missing="$(sort -u "$WORK/covered" | comm -13 - <(sort -u "$WORK/paths") | tr '\n' ',')"
  missing="${missing%,}"
  check "every command in hipc schema was run" "${missing:+missing: $missing}" test -z "$missing"
  # The token goes to grep through a pipe, not as an argument: a process's argv is readable
  # by every user on the machine. `printf` is a builtin, so it starts no process, and no
  # file holds the token.
  if grep -rlqF -f <(printf '%s' "$TOKEN") -- "$OUT"; then
    result FAIL "token never printed" "the token appears in captured output"
  else
    result PASS "token never printed" "checked $n runs"
  fi
  local esc
  esc="$({ grep -rlF -- $'\x1b' "$OUT" || true; } | wc -l | tr -d ' ')"
  check "no escape byte in any output" "$esc files" test "$esc" = 0
}

# group <function>: runs one group of checks in a subshell. Inside it `set -e` still stops
# the group at a command that fails outside a check, but not the run: that failure is one
# FAIL line, and the next group runs, so every group reports and the final token scan always
# comes. What the groups share in variables (the counters, the run number that names each
# capture, and the two account ids) comes back through $WORK/tally, which the subshell
# writes however it ends.
group() {
  local name="$1" status
  set +e
  (
    set -e
    trap 'printf "%s\n" "$pass" "$fail" "$skip" "$n" "$OWN" "$MEMBER" >"$WORK/tally"' EXIT
    "$name"
  )
  status=$?
  set -e
  {
    read -r pass
    read -r fail
    read -r skip
    read -r n
    read -r OWN
    read -r MEMBER
  } <"$WORK/tally"
  if [[ "$status" != 0 ]]; then
    result FAIL "$name" "stopped at a command that failed (exit $status); its later checks did not run"
  fi
}

for name in guard_checks baseline auth_checks account_checks storage_checks \
  compute_catalog_checks vm_checks database_checks kubernetes_checks app_engine_checks \
  cdn_checks support_checks network_checks membership_checks dry_run_checks \
  refusal_checks s3_configure_checks final_checks; do
  group "$name"
done

echo "== $pass passed, $fail failed, $skip skipped =="
[[ "$fail" == 0 ]]
