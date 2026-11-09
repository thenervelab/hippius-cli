#!/usr/bin/env bash
# A guided walkthrough of the hipc user journeys in docs/user-journeys.md, against the live API.
#
# Usage: HIPPIUS_TOKEN=<token> scripts/manual_test.sh [options] [scenario...]
#
#   --tier T           read (the default) runs reads only and shows every write as --dry-run;
#                      write also creates free resources and deletes them again; spend also
#                      buys what the run can undo, each after a y at the terminal
#   --yes-spend        answer y to every spend question (spend without a terminal needs it)
#   --hipc <path>      test this binary instead of downloading the latest release
#   --log <path>       where the transcript goes (default ./manual_test.log, overwritten)
#   --pause            wait for Enter after each step
#   --docker           in the write tier, also `docker login` with the new Hub key
#   --list             print the scenarios and their commands, run nothing
#   --excluded         print the commands no tier runs, each with its reason, run nothing
#   --offline          run every command against a dead origin with a fake token, so the
#                      script itself can be tested with no network; needs --hipc
#   scenario...        run only these scenarios (default: all; --list names them)
#
# The scenarios and their commands are documented in docs/user-journeys.md. Its command
# blocks are this script's `--list --tier write` output, and a test checks that they match,
# so change both together. Another test checks that every command `hipc schema` names is run
# by some tier, as itself or as a --dry-run, or is in EXCLUDED below with its reason, so a
# new command needs a step or a reason here.
#
# What the script promises the owner of the token:
#
# - Fresh start. Each scenario gets a new HOME, XDG_CONFIG_HOME, AWS config files and Docker
#   config under a temporary directory, logs in, and logs out at its end.
# - The token stays out of argv and out of the log. It is read from HIPPIUS_TOKEN once, then
#   unset, so no hipc process sees it in the environment; `hipc auth login --token-file -` reads
#   it from stdin. Every printed line goes through a sed mask (read with `sed -f`, so the
#   token is not in sed's argv either), and the run fails if the raw token, or a secret a
#   create revealed, is found in the log afterwards.
# - The read and write tiers leave the account as it was found. Reads are GETs. The read
#   tier writes nothing on the server: every write is a --dry-run, `network get` included,
#   since the first one for an account creates its network. The write tier creates only free
#   resources (an S3 bucket, an S3 sub token, an SSH key, a Hub key, a project, a CDN WAF
#   list), rotates the token and the key it made, and deletes each in the same scenario, as
#   a tested step. It also changes three things the account has (the runner concurrency, an
#   existing runner spend cap, a billing account's name) and puts each back, as a tested
#   step, skipping a change it could not put back; the ledger holds the value to put back.
#   In these two tiers anything that spends credits or touches a subscription runs only as
#   --dry-run.
# - The spend tier is opt-in: never the default, never implied by `write`. It runs all of
#   the write tier, and also buys, then undoes within the run: the cheapest Drive plan
#   (subscribe, then cancel; a cancel does not refund the period, so its first period is
#   paid), a support ticket (a person on the support team reads it; its subject marks it as
#   a CLI release test and asks the team to close it, and the run closes it itself when hipc
#   has `support tickets update`), the smallest VM, launched with --wait, with a public IP, a
#   power policy, an exposure and a backup policy (each undone, then the VM deleted), the
#   smallest database, and a CDN zone with a 1-credit cap on a new bucket. What it costs is
#   the Drive period plus the minutes the VM, the IP, the database and the zone run. Each
#   purchase prints its cost and waits for a y at the terminal; any other answer skips it and
#   every step that needs it. With stdin not a terminal the tier is refused unless
#   --yes-spend answers y for every purchase.
# - Every resource a write or spend step makes is entered in a cleanup ledger before its
#   create is sent, and the exit trap deletes (or cancels, or closes) whatever is left in it,
#   after a failure or Ctrl-C too. Before the first scenario and after the last, the script
#   takes a JSON snapshot of the account through read-only commands and diffs the two; any
#   difference, or a snapshot read that fails, fails the run. The spend tier allows exactly
#   the differences it made, and no other: the balance, the Drive subscription it cancelled,
#   and the tickets it opened.
#
# After a SIGKILL no trap runs. The ledger is then the recovery list: the run prints its path
# at the start (`$TMPDIR/hipc-manual.XXXXXX/ledger`, one `kind name id state` line per
# resource), and the directory, which also holds the stored token, is left behind; delete
# the resources it names by hand. A `runner-max`, `runner-cap` or `billing-name` line is not a
# resource but a change: put it back, from the value in `restore/<kind>.<id>` beside the
# ledger (for `runner-cap`, the body to send with `hipc api PUT /runners/limit/`). Then
# delete the directory.
#
# Written for bash 3.2, the macOS /bin/bash: no associative arrays, no mapfile, and empty
# arrays are expanded with ${a[@]+"${a[@]}"} under `set -u`.
set -euo pipefail

# Tracing would print the token the moment it is read, and every secret after it.
case "$-" in
*x*)
  echo "manual_test: refusing to run with xtrace (bash -x or SHELLOPTS=xtrace): it prints the token" >&2
  exit 2
  ;;
esac

# Byte semantics for sed and grep on hipc's raw output.
export LC_ALL=C
# Everything the script writes, the log included, is private to the user.
umask 077

REPO="thenervelab/hippius-cli"
# Port 9 is the discard port: nothing listens there, so every request is refused at once.
# hipc accepts plain http only for a loopback host, which this is. The test suite names
# another closed loopback port in HIPC_MANUAL_TEST_DEAD_ORIGIN, one it checked refuses
# connections; only a plain-http 127.0.0.1 origin with a port is taken.
DEAD_ORIGIN="http://127.0.0.1:9"
if [[ "${HIPC_MANUAL_TEST_DEAD_ORIGIN:-}" =~ ^http://127\.0\.0\.1:[1-9][0-9]{0,4}$ ]]; then
  DEAD_ORIGIN="$HIPC_MANUAL_TEST_DEAD_ORIGIN"
fi
OFFLINE_TOKEN="hipc-manual-test-offline-token-5f3a9c71"
BAD_TOKEN="hipc-manual-test-bad-token"
# A well-formed id that names nothing, for a delete whose create gave no id in --offline.
MISSING_UUID="0f8fad5b-d9cb-469f-a165-70867728950e"
MISSING_NUMBER="999999999"
MISSING_PROJECT="proj_00000000000000000000"
MISSING_ACCOUNT="acct_00000000000000000000"
MISSING_BILLING="bill_00000000000000000000"
SCENARIOS="first-run account-billing drive s3 hub compute cdn network-kubernetes"
SCENARIOS="$SCENARIOS team support agent token-hygiene"

TIER="read"
HIPC=""
LOG="$PWD/manual_test.log"
PAUSE=0
LIST=0
OFFLINE=0
DOCKER=0
EXCLUDED_ONLY=0
YES_SPEND=0
SELECTED=""

usage() {
  sed -n '4,18s/^# \{0,1\}//p' "$0"
}

die() {
  echo "manual_test: $*" >&2
  exit 2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
  --tier)
    [[ $# -ge 2 ]] || die "--tier needs read, write or spend"
    TIER="$2"
    shift 2
    ;;
  --hipc)
    [[ $# -ge 2 ]] || die "--hipc needs a path"
    HIPC="$2"
    shift 2
    ;;
  --log)
    [[ $# -ge 2 ]] || die "--log needs a path"
    LOG="$2"
    shift 2
    ;;
  --pause) PAUSE=1 && shift ;;
  --docker) DOCKER=1 && shift ;;
  --list) LIST=1 && shift ;;
  --excluded) EXCLUDED_ONLY=1 && shift ;;
  --yes-spend) YES_SPEND=1 && shift ;;
  --offline) OFFLINE=1 && shift ;;
  -h | --help) usage && exit 0 ;;
  -*) die "unknown option $1; run $0 --help" ;;
  *)
    [[ " $SCENARIOS " == *" $1 "* ]] || die "unknown scenario $1; one of: $SCENARIOS"
    # A scenario named twice runs once: its resources have one name per run.
    [[ " $SELECTED " == *" $1 "* ]] || SELECTED="$SELECTED $1"
    shift
    ;;
  esac
done
case "$TIER" in
read | write | spend) ;;
*) die "--tier is read, write or spend, not $TIER" ;;
esac
[[ -n "$SELECTED" ]] || SELECTED="$SCENARIOS"
# The spend tier asks before each purchase, at the terminal; with no terminal to ask at, it
# runs only when --yes-spend has answered for every purchase in advance.
if [[ "$TIER" == spend && $LIST == 0 && $EXCLUDED_ONLY == 0 && $YES_SPEND == 0 && ! -t 0 ]]; then
  die "--tier spend asks before each purchase and stdin is not a terminal; pass --yes-spend" \
    "to answer y to every one"
fi
# Without --hipc the release is downloaded with gh, which needs the network.
if [[ $OFFLINE == 1 && -z "$HIPC" && $LIST == 0 && $EXCLUDED_ONLY == 0 ]]; then
  die "--offline needs --hipc <path>: downloading the release needs the network"
fi
# 1 in the write and spend tiers, which create and delete; 1 in the spend tier alone, which
# also buys.
WRITES=0
SPENDS=0
[[ "$TIER" == read ]] || WRITES=1
[[ "$TIER" != spend ]] || SPENDS=1

# ---------------------------------------------------------------------------------------
# State of one run. In --list mode only the first group is used.

# The scenario now running, and its home directory: `~` in --list mode, so a command that
# names a file under it prints the same in both modes.
SC="setup"
H="~"
N=0
PASSES=0
FAILS=0
SKIPS=0
# Set by `step` for the step that follows, then cleared: a file for stdin and how to show
# it, a file under the home to copy stdout into, and why the step cannot run.
IN_FILE=""
IN_SHOW=""
OUT_TO=""
SKIP_REASON=""
# Why every step and check from here on is skipped, until it is cleared: a purchase was
# declined, or the resource a group of steps reads does not exist.
BLOCK_SKIP=""
# Set by `step`'s caller for the step that follows: how often a 425 is retried, and how many
# seconds apart (HIPC_MANUAL_RETRY_SECONDS, 10 by default; the tests set 0).
RETRY_425=0
RETRY_SECONDS="${HIPC_MANUAL_RETRY_SECONDS:-10}"
[[ "$RETRY_SECONDS" =~ ^[0-9]+$ ]] || die "HIPC_MANUAL_RETRY_SECONDS is a number of seconds"
# The outcome of the last step.
LAST_OUT=/dev/null
LAST_ERR=/dev/null
LAST_EXIT=0
WORK=""
# The secrets already masked, one per line, with a leading newline.
SECRETS_SEEN=$'\n'

# jq helpers shared by the checks, the snapshot and the cleanup.
#
# rows: the list a document holds. hipc prints most lists as {"items": [...]}, but a few
# pass the API's shape through ({"results": [...]}, {"zones": [...]}, a bare array); null
# when the document is not a list. An object with plan or balance fields is a document
# even when it holds an array of objects, so a subscription keeps its plan in the snapshot.
# complete: the list is every row there is, so a name missing from it is proof of absence.
# A list is incomplete when it names a next page, or when the API's `count` is larger than
# the rows it holds (`s3 buckets list` is one GET that follows no page link).
# revoked: a revoked S3 token, an archived project, or a VM, database or zone being deleted
# (a deleted VM or database is `decommissioning`, then `decommissioned`, and stays listed)
# can stay in the list; each is gone for every purpose here. The status is matched at its
# start, so `not_deleted` is live, and a status that names a failure (`delete_failed`,
# `termination_failed`) is never gone: the resource may still run and bill.
# shellcheck disable=SC2016 # jq source, not shell: the $ names are jq variables.
JQ_DEFS='
def rows:
  if type == "array" then .
  elif type != "object" then null
  else (.items // .results // .zones // .accounts // .projects // .billing_accounts //
        (if any(keys[]; IN("active", "status", "plan", "plan_id", "plan_code", "mode",
                           "balance"))
         then null
         else [.[] | arrays | select(length > 0 and (.[0] | type) == "object")] | first
         end) // null)
  end;
def complete:
  rows != null and
  (type != "object" or
   ((.next // null) == null and (.next_page_token // null) == null and
    ((.count | type) != "number" or .count == (rows | length))));
def revoked:
  type == "object" and
  (((.status // "") | tostring | ascii_downcase
    | test("^(deleting|deleted|terminat|decommission|revok|archiv)") and
      (test("fail") | not)) or
   .revoked == true or (.revoked_at // null) != null or .archived == true or
   (.archived_at // null) != null);
def row_name: .name // .bucket_name;
def live_names: [rows // [] | .[] | objects | select(revoked | not) | row_name];
def row_id: (.vm_id // .id // .name) | tostring;
'

# ---------------------------------------------------------------------------------------
# Output. Every line goes to the terminal and the log, through the mask.

say() {
  if [[ $LIST == 1 ]]; then
    printf '%s\n' "$*"
  else
    printf '%s\n' "$*" | sed -E -f "$WORK/mask.sed" | tee -a "$LOG"
  fi
}

# Prints a file hipc wrote, byte for byte apart from the mask, and a newline if it lacks one.
show_file() {
  if [[ ! -s "$1" ]]; then
    say "(empty)"
    return
  fi
  sed -E -f "$WORK/mask.sed" "$1" | tee -a "$LOG"
  if [[ -n "$(tail -c 1 "$1")" ]]; then
    say ""
  fi
}

# A regex that matches the bytes of $1 in sed -E, with `/` escaped for the s command.
sed_escape() {
  local s="$1" out="" c i
  for ((i = 0; i < ${#s}; i++)); do
    c="${s:i:1}"
    case "$c" in
    \\ | '/' | '.' | '*' | '^' | '$' | '[' | ']' | '+' | '?' | '(' | ')' | '{' | '}' | '|')
      out="$out\\$c"
      ;;
    *) out="$out$c" ;;
    esac
  done
  printf '%s' "$out"
}

# Adds one line of bytes to the mask and to the list the final log scan looks for, when it
# is at least $3 bytes long (default 8). A value from a known secret field passes 1: a short
# password is still a password. A guess (one line of a multi-line secret) keeps 8, since
# masking every short match would garble the transcript. The pattern is only ever passed to
# shell builtins and to programs that read it from a file: a grep for it would put it in a
# process's argv.
#
# The literal rules run longest first. A short secret that is part of a longer one (a
# password that is a prefix of the passphrase) would otherwise replace its part of the
# longer one first, and leave the rest of it in the log, where no rule matches it any more.
# An escaped pattern holds the escape of every part of it, so ordering by the escaped length
# puts each secret before every secret it contains.
add_pattern() {
  local pattern="$1" label="$2" min="${3:-8}" escaped
  [[ -n "$pattern" && ${#pattern} -ge $min ]] || return 0
  [[ "$SECRETS_SEEN" != *$'\n'"$pattern"$'\n'* ]] || return 0
  SECRETS_SEEN="$SECRETS_SEEN$pattern"$'\n'
  printf '%s\n' "$pattern" >>"$WORK/secrets"
  # Where it came from, for the log scan's report; the label and step name no value.
  printf 'learned by the mask at step %s as %s\n' "$N" "$label" >>"$WORK/secrets.src"
  escaped="$(sed_escape "$pattern")"
  printf '%s s/%s/%s/g\n' "${#escaped}" "$escaped" "$label" >>"$WORK/mask.rules"
  build_mask
}

# Writes $WORK/mask.sed: the fixed rules (the token preview), the literal rules longest
# first, then the rules that mask by shape (a password in a query string). Those come last,
# so they cannot break up a literal secret before its own rule runs.
build_mask() {
  {
    cat "$WORK/mask.head"
    sort -s -k1,1nr "$WORK/mask.rules" | cut -d ' ' -f 2-
    cat "$WORK/mask.tail"
  } >"$WORK/mask.sed.new"
  mv "$WORK/mask.sed.new" "$WORK/mask.sed"
}

# add_secret <secret> <label> [min]: masks a secret in the two forms the log can hold it:
# the bytes, as text mode prints them, and the JSON string escape, as a JSON document prints
# them. sed and grep match within one line, so a secret with a line break is also masked
# line by line, each line held to 8 bytes; its JSON form is one line, and is longer than
# any of its lines, so add_pattern's order masks it before them.
add_secret() {
  local secret="$1" label="$2" min="${3:-8}" part
  add_pattern "$(json_escape "$secret")" "$label" "$min"
  if [[ "$secret" != *$'\n'* ]]; then
    add_pattern "$secret" "$label" "$min"
    return 0
  fi
  while IFS= read -r part; do
    add_pattern "$(json_escape "$part")" "$label"
    add_pattern "$part" "$label"
  done <<<"$secret"
}

# The JSON string escape of $1, without the quotes. jq reads it from stdin, so it is not in
# jq's argv.
json_escape() {
  local escaped
  escaped="$(printf '%s' "$1" | jq -Rs .)"
  printf '%s' "${escaped:1:${#escaped}-2}"
}

# Every string a JSON answer holds under a secret field, one JSON string per line: a key
# that names a secret, a password, a passphrase, a private or client key, a download link,
# or a checkout URL; a connection string or DSN that holds a password (`password=` or a
# password in its user part: `postgresql://app@host/app` holds none, and hipc prints it on
# purpose); and a URL or URI whose query carries a
# signature, a token, a password or a passphrase (a presigned URL reads an object without
# the console token), or whose user part holds a password (`postgres://user:pass@host`). A
# dry run's request `url` has none of these, so it stays readable. `jq -c` keeps a line
# break inside a string on one line.
#
# The fields of every command that takes `--reveal` are here: an S3 key's `secret`, the
# Hub `robot_secret` and key `secret`, a database's `password`, `backup_cipher_passphrase`
# and TLS `client_key`, a presigned `url`, and a Kubernetes backup's `download_url` and
# `manifest.sealed_key`; and, for the routes `hipc api --reveal` guards, the master
# `token`, `access_token`, `refresh_token`, a NetBird `setup_key` and `join_command`, and a
# Stripe `client_secret`. A VM shell `ticket` is not: a support message names its ticket
# under that key, and the script never asks for a shell ticket. Nor is a `*_preview` (an S3
# key's `secret_preview`, the `token_preview`): hipc prints it on purpose, in text tables
# too, where no rule can find it, so taking it for a secret only fails the log scan.
#
# SECRET_ENTRIES gives each as a `{key, value}` entry, for the log scan's report of where a
# needle came from.
# shellcheck disable=SC2016 # jq source.
#
# A value is read with its percent-encoded `:`, `@` and `=` decoded first, and a user part
# may be empty (`postgresql://:pw@host`): when in doubt it is a secret, since a false
# positive only fails the run and a miss leaks.
SECRET_ENTRIES='def decoded: gsub("%3[Aa]"; ":") | gsub("%40"; "@") | gsub("%3[Dd]"; "=");
  .. | objects | to_entries[]
  | select(.key | ascii_downcase | endswith("_preview") | not)
  | select((.key | ascii_downcase
      | test("secret|password|passphrase|private_key|client_key" +
          "|download|sealed_key|setup_key|join_command|^token$|^access_token$" +
          "|^refresh_token$|^checkout_url$"))
    or ((.key | ascii_downcase | test("connection_string|dsn")) and
      (.value | type) == "string" and
      (.value | decoded
        | test("(^|[?&; ])(password|passphrase|pass|pwd)=|://[^/@:]*:[^/@]+@"; "i")))
    or ((.key | ascii_downcase | test("(url|uri)$")) and (.value | type) == "string" and
      (.value | decoded | test("[?&;](x-amz-signature|signature|sig|token|key|password" +
        "|passphrase|pwd|pass|secret)=" +
        "|://[^/@:]*:[^/@]+@"; "i"))))
  | select(.value | type == "string")'
SECRET_FIELDS="$SECRET_ENTRIES | .value"
secret_values() { jq -c "$SECRET_FIELDS" "$1" 2>/dev/null || true; }

# Masks every secret field of a JSON answer (on stdout or stderr) before it is shown.
#
# The values go through a file, not a process substitution: at the end of a whole offline
# run, bash 3.2 (the macOS /bin/bash) spun on the CPU for minutes in the log scan's loops
# over hundreds of process substitutions, so the script runs none per step.
mask_secrets_in() {
  local secret
  secret_values "$1" >"$WORK/values"
  [[ -s "$WORK/values" ]] || return 0
  while IFS= read -r secret; do
    add_secret "$(jq -r . <<<"$secret")" "<redacted>" 1
  done <"$WORK/values"
}

# One word of a command line as a POSIX shell reads it: as it is when it holds only safe
# characters (and the <placeholder> brackets of --list), else in single quotes.
quote_word() {
  local quote="'"
  case "$1" in
  '') printf "''" ;;
  *[!A-Za-z0-9_./:=@+,%~\<\>-]*) printf "'%s'" "${1//$quote/$quote\\$quote$quote}" ;;
  *) printf '%s' "$1" ;;
  esac
}

# How a command line is shown: `hipc` for the binary under test, a path under the scenario's
# home as `~/...`, and every other word quoted for a POSIX shell.
show_argv() {
  local line="" word
  for word in "$@"; do
    if [[ $LIST == 0 && "$word" == "$H/"* ]]; then
      line="$line ~/$(quote_word "${word#"$H/"}")"
    else
      line="$line $(quote_word "$word")"
    fi
  done
  printf '%s' "${line# }"
}

record() {
  local verdict="$1" what="$2"
  case "$verdict" in
  PASS) PASSES=$((PASSES + 1)) ;;
  FAIL)
    FAILS=$((FAILS + 1))
    printf '[%s] %s: %s\n' "$N" "$SC" "$what" >>"$WORK/failures"
    ;;
  SKIP) SKIPS=$((SKIPS + 1)) ;;
  esac
}

pause() {
  if [[ $PAUSE == 1 && $LIST == 0 ]]; then
    read -r -p "press Enter for the next step " </dev/tty
  fi
}

# ---------------------------------------------------------------------------------------
# Expectations.
#
#   ok           exit 0, online and offline: a local command or a --dry-run
#   net          exit 0; offline, `unavailable`: a read that needs the API
#   net/C1,C2    exit 0 or one of these codes; offline, `unavailable` or one of these codes
#   err:C        the error code C with its exit code, online and offline
#   net-err:C    C; offline, `unavailable`

# The error code of a failed run: the JSON error's `code`, or, in text mode, `unavailable`
# when the connection was refused.
#
# An account whose private network was never set up answers every network read 403, "You
# must call /api/network/provision/ first."; hipc keeps that in `details.detail`, and since
# `hipc network create` its message ends "the private network is not set up for this
# account". That 403 `permission` is the state `network_unprovisioned` here, which the
# network steps and the snapshot accept. It is matched on the server's provision detail
# (in `details`, or quoted in the message of an older hipc) or on that exact message, never
# on the hint: a 403 for a role that may not read the network can still name the create in
# its hint, and is a real failure. Setting the network up cannot be undone, so no tier does
# it.
#
# An account with no Hub namespace answers the namespace's reads, its keys and repositories
# 403, "You must call /api/registry/provision/ first.", which hipc reports as `permission`
# with a hint naming `hipc hub namespace create`. That is the state `hub_unprovisioned`,
# matched on that whole sentence in the message or `details`, never on the hint or on the
# path alone: "Only an owner may call /api/registry/provision/." is a real refusal. The
# network's sentence is matched whole the same way.
# Creating a namespace claims a name for good, so no tier does it either.
error_code() {
  local code
  code="$(jq -er '.error.code' "$1" 2>/dev/null || true)"
  if [[ -z "$code" ]] && grep -q 'was refused' "$1" 2>/dev/null; then
    code=unavailable
  elif [[ "$code" == permission ]] &&
    jq -e '.error | (.http_status // 403) == 403 and
      (([.message, (.details | tostring)] | map(strings) | join(" ")
        | test("You must call /api/network/provision/ first\\.")) or
       ((.message | type) == "string" and
        (.message | endswith("the private network is not set up for this account"))))' \
      "$1" >/dev/null 2>&1; then
    code=network_unprovisioned
  elif [[ "$code" == permission ]] &&
    jq -e '.error | (.http_status // 403) == 403 and
      ([.message, (.details | tostring)] | map(strings) | join(" ")
        | test("You must call /api/registry/provision/ first\\."))' "$1" >/dev/null 2>&1; then
    code=hub_unprovisioned
  fi
  [[ -z "$code" ]] || printf '%s\n' "$code"
}

exit_of() {
  awk -v code="$1" '$1 == code { print $2 }' "$WORK/exits"
}

# Sets WANT to the expectation in words; returns 0 when the run met it.
judge() {
  local expect="$1" status="$2" err="$3" zero=1 codes="" code
  case "$expect" in
  ok) ;;
  net) [[ $OFFLINE == 0 ]] || { zero=0 && codes="unavailable"; } ;;
  # Offline a listed code is accepted beside `unavailable`: a stand-in for hipc may answer it.
  net/*)
    codes="${expect#net/}"
    if [[ $OFFLINE == 1 ]]; then zero=0 && codes="unavailable,$codes"; fi
    ;;
  err:*) zero=0 && codes="${expect#err:}" ;;
  net-err:*)
    zero=0
    if [[ $OFFLINE == 1 ]]; then codes="unavailable"; else codes="${expect#net-err:}"; fi
    ;;
  *) die "bug: unknown expectation $expect" ;;
  esac
  WANT=""
  [[ $zero == 0 ]] || WANT="exit 0"
  if [[ -n "$codes" ]]; then
    WANT="${WANT:+$WANT or }error code $codes"
  fi
  if [[ "$status" == 0 ]]; then
    [[ $zero == 1 ]]
    return
  fi
  [[ -n "$codes" ]] || return 1
  code="$(error_code "$err")"
  [[ -n "$code" && ",$codes," == *",$code,"* && "$status" == "$(exit_of "$code")" ]]
}

# ---------------------------------------------------------------------------------------
# Steps.

# step <tag> <expectation> <title> <why> -- [VAR=value...] <program> [args...]
#
# <tag> says what the command does to the account: setup, read, local (a file on this
# machine), dry-run, write (creates on the server), undo (deletes what a write created),
# error (a refusal on purpose), ext (another program). `hipc` as the program is the binary
# under test.
step() {
  local tag="$1" expect="$2" title="$3" why="$4" shown status
  shift 5
  local -a envs=()
  while [[ $# -gt 0 && "$1" == [A-Z]*=* ]]; do
    envs+=("$1")
    shift
  done
  shown="$(show_argv ${envs[@]+"${envs[@]}"} "$@")${IN_SHOW:+ $IN_SHOW}"
  [[ -z "$OUT_TO" ]] || shown="$shown > $(show_argv "$OUT_TO")"
  # Only a JSON error carries a code, so a step that accepts one must not ask for text.
  case "$expect" in
  net/* | err:* | net-err:*)
    [[ " $* " != *" --output text "* ]] || die "bug: $title expects an error code but asks for text"
    ;;
  esac
  if [[ $LIST == 1 ]]; then
    printf '  [%s] %s\n' "$tag" "$shown"
    IN_FILE="" IN_SHOW="" OUT_TO="" SKIP_REASON="" RETRY_425=0
    return 0
  fi

  N=$((N + 1))
  say ""
  say "==== [$N] $SC: $title ($tag)"
  say "why: $why"
  say "\$ $shown"
  [[ -n "$SKIP_REASON" ]] || SKIP_REASON="$BLOCK_SKIP"
  if [[ -n "$SKIP_REASON" ]]; then
    say "SKIP: $SKIP_REASON"
    record SKIP "$title"
    IN_FILE="" IN_SHOW="" OUT_TO="" SKIP_REASON="" RETRY_425=0
    LAST_OUT=/dev/null LAST_ERR=/dev/null LAST_EXIT=skipped
    pause
    return 0
  fi

  local -a argv=("$@")
  [[ "${argv[0]}" != hipc ]] || argv[0]="$HIPC"
  LAST_OUT="$WORK/steps/$N.out"
  LAST_ERR="$WORK/steps/$N.err"
  local tries=0
  while :; do
    set +e
    env ${envs[@]+"${envs[@]}"} "${argv[@]}" <"${IN_FILE:-/dev/null}" >"$LAST_OUT" 2>"$LAST_ERR"
    status=$?
    set -e
    # RETRY_425=N retries a 425 (`still_confirming`: the purchase it undoes is not settled
    # yet) N times, RETRY_SECONDS apart.
    [[ $status != 0 && $tries -lt $RETRY_425 ]] || break
    jq -e '.error
      | .http_status == 425 or ((.details // {}) | tostring | test("still_confirming"))' \
      "$LAST_ERR" >/dev/null 2>&1 || break
    tries=$((tries + 1))
    say "--- 425: still confirming; try $((tries + 1)) of $((RETRY_425 + 1))" \
      "in $RETRY_SECONDS seconds"
    sleep "$RETRY_SECONDS"
  done
  RETRY_425=0
  LAST_EXIT=$status
  mask_secrets_in "$LAST_OUT"
  mask_secrets_in "$LAST_ERR"
  if [[ -n "$OUT_TO" ]]; then
    cp "$LAST_OUT" "$OUT_TO"
  fi

  say "--- stdout ---"
  show_file "$LAST_OUT"
  say "--- stderr ---"
  show_file "$LAST_ERR"
  if judge "$expect" "$status" "$LAST_ERR"; then
    say "--- exit $status; expected $WANT: PASS"
    record PASS "$title"
    if [[ $status != 0 && "$(error_code "$LAST_ERR")" == network_unprovisioned ]]; then
      say "note: this account's private network is not set up; setting it up cannot be undone," \
        "so no tier does it"
    fi
    if [[ $status != 0 && "$(error_code "$LAST_ERR")" == hub_unprovisioned ]]; then
      say "note: this account has no Hub namespace; creating one claims a name for good, so" \
        "no tier does it"
    fi
  else
    say "--- exit $status; expected $WANT: FAIL"
    record FAIL "$title (exit $status; expected $WANT)"
  fi
  IN_FILE="" IN_SHOW="" OUT_TO="" SKIP_REASON=""
  pause
}

# check <online|always> <title> -- <function> [args...]: an assertion the script makes on
# what the last step left behind. An `online` check needs a live answer, so --offline skips
# it.
check() {
  local when="$1" title="$2"
  shift 3
  if [[ $LIST == 1 ]]; then
    printf '  [check] %s\n' "$title"
    return 0
  fi
  if [[ -n "$BLOCK_SKIP" ]]; then
    say "check: $title: SKIP ($BLOCK_SKIP)"
    record SKIP "$title"
  elif [[ $OFFLINE == 1 && "$when" == online ]]; then
    say "check: $title: SKIP (offline)"
    record SKIP "$title"
  elif "$@"; then
    say "check: $title: PASS"
    record PASS "$title"
  else
    say "check: $title: FAIL"
    record FAIL "check: $title"
  fi
}

# The checks.
listed() {
  jq -e --arg n "$1" "$JQ_DEFS"' live_names | any(.[]; . == $n)' "$LAST_OUT" >/dev/null &&
    return 0
  say "listed: \`$1\` is not among the $(jq "$JQ_DEFS"' live_names | length' "$LAST_OUT" \
    2>/dev/null || echo 0) live rows of the list (a row's name is its name or bucket_name)"
  return 1
}
# Absence needs a complete list: a name missing from one page proves nothing.
not_listed() {
  jq -e --arg n "$1" "$JQ_DEFS"' complete and (live_names | any(.[]; . == $n) | not)' \
    "$LAST_OUT" >/dev/null
}
no_token_in() { [[ ! -e "$1" ]] || ! grep -qF -f "$WORK/token" -- "$1"; }
config_private() {
  [[ -n "$(find "$H/.hippius/config.toml" -maxdepth 0 -type f -perm 600)" ]]
}
schema_lists_commands() {
  jq -e '(.commands | length) > 100 and (.errors | length) > 10' "$LAST_OUT" >/dev/null
}
# On success the answer is JSON on stdout; on failure, one JSON object on stderr.
answer_is_json() {
  if [[ $LAST_EXIT == 0 ]]; then
    jq -e . "$LAST_OUT" >/dev/null
  else
    jq -e '.error.code' "$LAST_ERR" >/dev/null
  fi
}
stdout_is_array() { jq -e 'type == "array"' "$LAST_OUT" >/dev/null; }
# Whether the jq filter $1 is true of the last step's JSON answer.
last_is() { jq -e "$1" "$LAST_OUT" >/dev/null 2>&1; }

# A value read from the last step's JSON answer: the placeholder in --list mode, else the
# first match of the jq filter, or, when there is none, the fallback in --offline (where
# every read fails) and nothing online.
pick() {
  local placeholder="$1" fallback="$2" filter="$3" value
  if [[ $LIST == 1 ]]; then
    printf '%s' "$placeholder"
    return 0
  fi
  value="$(answered "$filter")"
  [[ -n "$value" || $OFFLINE == 0 ]] || value="$fallback"
  printf '%s' "$value"
}

# pick, but with no fallback when the last step answered: a value it does not hold is not
# there, offline too (where only a stand-in for hipc answers).
pick_answered() {
  if [[ $LIST == 0 && "$LAST_EXIT" == 0 ]]; then
    answered "$3"
  else
    pick "$@"
  fi
}

# The first match of the jq filter in the last step's answer, with no fallback: the id a
# create really answered, which the ledger may record.
answered() {
  [[ $LIST == 0 ]] || return 0
  jq -r "$JQ_DEFS $1 | select(. != null) | tostring" "$LAST_OUT" 2>/dev/null | head -n 1 || true
}

# ---------------------------------------------------------------------------------------
# The cleanup ledger: one `kind name id state` line per resource a write tier step creates.
# It is written before the create is sent (id `-`, state `pending`), so a create that times
# out after the server acted is cleaned up too. Once the create exits 0 the line records the
# id it answered and the state `created`. A line is removed once the scenario's own delete
# exited 0 and a complete list no longer shows the name.

# A create that is skipped (a declined purchase, a missing resource) makes nothing, so it
# gets no line.
ledger_add() {
  [[ $LIST == 1 || -n "$BLOCK_SKIP" ]] || printf '%s %s - pending\n' "$1" "$2" >>"$WORK/ledger"
}

# Rewrites the ledger through awk; the names and ids it is given are not secrets.
ledger_edit() {
  awk "$@" "$WORK/ledger" >"$WORK/ledger.new"
  mv "$WORK/ledger.new" "$WORK/ledger"
}

# ledger_created <kind> <name> <create exit status> <id>
ledger_created() {
  [[ $LIST == 0 && "$3" == 0 ]] || return 0
  # shellcheck disable=SC2016 # awk source: $1 to $4 are awk fields.
  ledger_edit -v k="$1" -v n="$2" -v id="${4:--}" \
    '$1 == k && $2 == n { $3 = id; $4 = "created" } { print }'
}

# ledger_answered <kind> <name> <id>: a create that did not exit 0 can still have made the
# resource, and a VM launch whose wait ends early prints its id (tenet 3); an id on stdout is
# recorded, so the exit trap deletes by it.
ledger_answered() {
  [[ $LIST == 0 && -n "$3" ]] || return 0
  ledger_created "$1" "$2" 0 "$3"
}

# ledger_rename <kind> <old name> <new name> <id>: one in-place edit, so there is no moment
# when the resource has no line.
ledger_rename() {
  [[ $LIST == 0 ]] || return 0
  # shellcheck disable=SC2016 # awk source: $1 to $4 are awk fields.
  ledger_edit -v k="$1" -v o="$2" -v n="$3" -v id="${4:--}" \
    '$1 == k && $2 == o { $2 = n; $3 = id; $4 = "created" } { print }'
}

# ledger_mark <kind> <name> <state>: a Drive cancel that exited 0 makes its line
# `cancelled`, so the exit trap reads the plan again and sends no second cancel.
ledger_mark() {
  [[ $LIST == 0 ]] || return 0
  # shellcheck disable=SC2016 # awk source: $1, $2 and $4 are awk fields.
  ledger_edit -v k="$1" -v n="$2" -v s="$3" '$1 == k && $2 == n { $4 = s } { print }'
}

ledger_remove() {
  # shellcheck disable=SC2016 # awk source: $1 and $2 are awk fields.
  ledger_edit -v k="$1" -v n="$2" '!($1 == k && $2 == n)'
}

# The state the ledger holds for a line: pending, created, or empty when there is none.
ledger_state() {
  [[ $LIST == 0 ]] || return 0
  awk -v k="$1" -v n="$2" '$1 == k && $2 == n { print $4 }' "$WORK/ledger"
}

# bought <create exit status> [id]: the purchase spent money when its create exited 0, or
# answered an id anyway; only then may the balance move.
bought() {
  [[ $LIST == 0 ]] || return 0
  if [[ "$1" == 0 || -n "${2:-}" ]]; then
    SPENT=1
  fi
}

# Removes the ledger line when the scenario's delete (whose exit status is $3) passed and
# the complete list read after it no longer shows the name.
ledger_settle() {
  [[ $LIST == 0 && $OFFLINE == 0 && "$3" == 0 ]] || return 0
  if not_listed "$2"; then
    ledger_remove "$1" "$2"
  fi
}

# The write tier also changes a few things the account already has (a runner setting, the
# runner spend cap, a peer's name, a billing account's name) and puts each back, so a live
# run shows whether the field hipc sends is the one the server keeps. Before the change is
# sent, the ledger gets a `<kind> <id> - pending` line and the value to put back goes to
# $WORK/restore/<kind>.<id>, so the exit trap puts it back after a failure or Ctrl-C too.
# change_begin <kind> <id> <value to put back>
change_begin() {
  [[ $LIST == 0 && -z "$BLOCK_SKIP" ]] || return 0
  printf '%s\n' "$3" >"$WORK/restore/$1.$2"
  printf '%s %s - pending\n' "$1" "$2" >>"$WORK/ledger"
}

# shows <jq filter> <id> <value>: whether the last step's answer shows the value, with the
# filter given $id and $v.
shows() {
  jq -e --arg id "$2" --arg v "$3" "$JQ_DEFS $1" "$LAST_OUT" >/dev/null 2>&1
}

# change_settle <kind> <id> <put-back exit status> <jq filter> <value>: removes the line when
# the scenario's own put-back exited 0 and the read after it, the last step, shows the value.
change_settle() {
  [[ $LIST == 0 && $OFFLINE == 0 && "$3" == 0 ]] || return 0
  if shows "$4" "$2" "$5"; then
    ledger_remove "$1" "$2"
  fi
}

# An undo runs only after this run's own create exited 0 and answered an id: a delete by a
# guessed name could remove a resource this run did not make. Offline it runs anyway,
# against the dead origin, so the step itself is exercised.
undo_needs() {
  local created="$1" id="$2"
  [[ $LIST == 0 && $OFFLINE == 0 ]] || return 0
  if [[ "$created" != 0 ]]; then
    SKIP_REASON="the create did not exit 0; the cleanup ledger looks for it by name"
  elif [[ -z "$id" ]]; then
    SKIP_REASON="the create answered no id; the cleanup ledger looks for it by name"
  fi
}

# A group of steps that reads one resource runs only when there is one: with an empty id,
# every step and check until block_end is skipped, with the reason. In --list mode the
# group is always listed.
block_needs() {
  local id="$1" reason="$2"
  [[ $LIST == 1 || -n "$id" ]] || BLOCK_SKIP="$reason"
}
block_end() { BLOCK_SKIP=""; }

# ---------------------------------------------------------------------------------------
# The spend tier's purchases.

# spend_gate <what it buys> <what it costs>: prints the cost and asks for a y on stdin, the
# terminal, or takes --yes-spend's. A declined purchase skips every step until block_end, so
# nothing that needs it runs, and its ledger line is never written. Sets SPEND_OK to 1 when
# the purchase may go ahead.
SPEND_OK=0
SPENT=0
spend_gate() {
  local what="$1" cost="$2" answer=""
  SPEND_OK=0
  if [[ $LIST == 1 ]]; then
    printf '  [ask] %s: %s\n' "$what" "$cost"
    SPEND_OK=1
    return 0
  fi
  say ""
  say "==== $SC: spend: $what"
  say "cost: $cost"
  if [[ $YES_SPEND == 1 ]]; then
    say "answer: y (--yes-spend)"
    SPEND_OK=1
  else
    # stdin is a terminal here: the tier is refused at the start otherwise. The question
    # goes to the terminal, not the log; the answer is logged below. Every step runs with
    # its own stdin, so only this read takes what is typed.
    read -r -p "type y to buy it, anything else to skip it: " answer || answer=""
    if [[ "$answer" == y || "$answer" == Y ]]; then
      say "answer: y"
      SPEND_OK=1
    else
      say "answer: no; the steps that need it are skipped"
    fi
  fi
  # SPENT is set by `bought`, once the create exits 0, not by the answer.
  if [[ $SPEND_OK == 0 ]]; then
    BLOCK_SKIP="the purchase was declined"
    record SKIP "spend: $what"
  fi
}

# Whether the binary under test has a command, from its schema; the spend tier uses it for
# a step a newer hipc adds, so the script runs against an older release too.
# In --list mode it asks the binary --hipc names, and is false without one, so the listing
# never names a command the binary lacks.
has_command() {
  [[ $LIST == 0 ]] || command -v "$HIPC" >/dev/null || return 1
  "$HIPC" schema </dev/null 2>/dev/null |
    jq -e --arg p "$1" 'any(.commands[]; (.path | join(" ")) == $p)' >/dev/null
}

# ---------------------------------------------------------------------------------------
# Scenario homes, login and logout.

begin_scenario() {
  SC="$1"
  if [[ $LIST == 1 ]]; then
    H="~"
    printf '\nscenario %s: %s\n' "$1" "$2"
    return 0
  fi
  H="$WORK/scenarios/$1/home"
  mkdir -p "$H"
  enter_home "$H"
  say ""
  say "######## scenario $1: $2"
  say "home: a new, empty directory (HOME, XDG_CONFIG_HOME, AWS and Docker config)"
}

end_scenario() {
  [[ $LIST == 1 ]] || say "######## end of scenario $SC"
}

enter_home() {
  export HOME="$1"
  export XDG_CONFIG_HOME="$1/.config"
  export AWS_CONFIG_FILE="$1/.aws/config"
  export AWS_SHARED_CREDENTIALS_FILE="$1/.aws/credentials"
  export DOCKER_CONFIG="$1/.docker"
}

login() {
  local -a extra=()
  # A login for any origin but https://api.hippius.com asks first (tenet 9).
  [[ $OFFLINE == 0 ]] || extra=(--yes)
  IN_FILE="$WORK/token"
  IN_SHOW="< (the token, on stdin)"
  step local ok "Log in" \
    "store the console token in the profile; --token-file - reads it from stdin, so it is never in argv" \
    -- hipc --output text auth login --token-file - ${extra[@]+"${extra[@]}"}
}

# The login stores the token without checking it, so a wrong or expired one would fail every
# API step with the same 401. One authenticated read, in the snapshot home, checks it before
# any scenario, and a refusal stops the run there. The token comes only from HIPPIUS_TOKEN;
# a profile's stored token is never read.
check_token() {
  if [[ $OFFLINE == 1 ]]; then
    say "token: the offline stand-in, not checked: there is no API to ask"
    return 0
  fi
  say "token: from HIPPIUS_TOKEN"
  if "$HIPC" --json auth status </dev/null >"$WORK/token-check.out" 2>"$WORK/token-check.err"; then
    say "token: accepted (hipc auth status)"
    return 0
  fi
  local code
  code="$(error_code "$WORK/token-check.err")"
  mask_secrets_in "$WORK/token-check.err"
  show_file "$WORK/token-check.err"
  say "token: the token from HIPPIUS_TOKEN cannot be used: \`hipc auth status\` failed" \
    "(${code:-unknown}); no scenario ran. Create a token in the console settings, or check the" \
    "network, and run again."
  record FAIL "the token from HIPPIUS_TOKEN cannot be used (${code:-unknown})"
  exit 1
}

logout() {
  step local ok "Log out" "remove the stored token from this machine" \
    -- hipc --output text auth logout --yes
  check always "the config file no longer holds the token" -- no_token_in "$H/.hippius/config.toml"
}

# ---------------------------------------------------------------------------------------
# Scenarios. Each one is a straight list of steps; `--list` runs the same functions and
# prints instead of running.

scenario_first_run() {
  begin_scenario first-run "install, read the version and help, log in, check, log out"
  step local ok "Print the version" "confirm the installed binary runs and which release it is" \
    -- hipc --output text version
  step local ok "Read the help" "see the services and the global flags" -- hipc --output text --help
  step error err:auth "Check the login before logging in" \
    "a fresh install has no token; hipc says so and names the login command" \
    -- hipc --json auth status
  login
  step read net "Check the login" "confirm the token works and see whose it is" \
    -- hipc --output text auth status
  step read net "List the accounts" "see the own account and every shared one" \
    -- hipc --output text account list
  step read net "Read the product status" "see the state of Hub, Drive, S3 and compute" \
    -- hipc --output text status
  logout
  step error err:auth "Check the login after logging out" \
    "logging out leaves no token, so hipc refuses again" -- hipc --json auth status
  end_scenario
}

scenario_account_billing() {
  local invoice referral
  begin_scenario account-billing "read the account, its members, the balance and the bills"
  login
  step read net "Read the account" "see the selected account" -- hipc --output text account get
  step read net "Read the notifications" "see which emails the signed-in user gets" \
    -- hipc --output text account notifications get
  step dry-run ok "Preview a notification change" \
    "see the request; it would change which emails the owner gets" \
    -- hipc --output text account notifications update --low-credit-alerts true --dry-run
  step read net "Read the marketing consent" "see whether the user gets marketing email" \
    -- hipc --output text account marketing-consent get
  step dry-run ok "Preview withdrawing the consent" \
    "see the request; a withdrawal stays in the consent history, and only the console gives it" \
    -- hipc --output text account marketing-consent update --granted false --dry-run
  step read net "List the members" "see who can act in the account" \
    -- hipc --output text account members list
  # A GET mints nothing: the API documents that only the create mints the user's code, on
  # first use.
  step read net "List the referral links" "see the links that share the referral code" \
    -- hipc --json account referrals list
  referral="$(pick "<referral>" "$MISSING_NUMBER" 'first(rows // [] | .[] | .id)')"
  step read net "Read the referral analytics" "see the signups and top-ups the links brought" \
    -- hipc --output text account referrals analytics
  # A link has no delete, and the first create mints the user's code for good, so both
  # writes are dry runs in every tier.
  step dry-run ok "Preview a referral link" "see the request; a link cannot be deleted" \
    -- hipc --output text account referrals create --name hipc-manual --dry-run
  step dry-run ok "Preview renaming a referral link" "see the request; nothing is renamed" \
    -- hipc --output text account referrals update "${referral:-$MISSING_NUMBER}" \
    --name hipc-manual --dry-run
  step read net "Read the balance" "see the credits left and the runway" \
    -- hipc --output text billing balance
  step read net "Read the legacy Stripe plan" "see the old top-up plan, if there is one" \
    -- hipc --output text billing stripe
  step read net "List the subscriptions" "see every paid plan in one place" \
    -- hipc --output text billing subscriptions
  step read net "List recent transactions" "see what was charged and credited" \
    -- hipc --output text billing transactions --limit 5
  step read net "Read the billing summary" "see the month's spend by product" \
    -- hipc --output text billing summary
  step read net "Read the usage" "see the metered usage this month" \
    -- hipc --output text billing usage
  step read net "List recent invoices" "find an invoice to download" \
    -- hipc --json billing invoices list --limit 5
  invoice="$(pick "<invoice>" "$MISSING_NUMBER" 'first(rows // [] | .[] | .id)')"
  [[ -n "$invoice" ]] || SKIP_REASON="the account has no invoice"
  step read net "Download an invoice" "save the first invoice's PDF in the temporary home" \
    -- hipc --output text billing invoices get "$invoice" --output-file "$H/invoice.pdf"
  step read net "Read the billing profile" "see the name and address the invoices carry" \
    -- hipc --output text billing profile get
  step dry-run ok "Preview a billing profile change" \
    "see the request; it would change what the next invoice says" \
    -- hipc --output text billing profile update --city Paris --dry-run
  step read net "Read the auto-reload settings" "see whether the card tops up the balance" \
    -- hipc --output text billing auto-reload get
  step read net "Read the auto-reload history" "see past automatic top-ups" \
    -- hipc --output text billing auto-reload history --limit 5
  step dry-run ok "Preview turning auto-reload off" \
    "see the request; it would change when the card is charged" \
    -- hipc --output text billing auto-reload update --enabled false --dry-run
  step dry-run ok "Preview saving a card" \
    "see the request; it starts a Stripe checkout that a person completes" \
    -- hipc --output text billing auto-reload card-setup --dry-run
  step dry-run ok "Preview a top-up" "see the checkout request a top-up sends; it spends money" \
    -- hipc --output text billing top-up --amount 10 --method card --dry-run
  step read net "Read the hAlpha quote" "see the hAlpha price and what a hAlpha top-up would buy" \
    -- hipc --output text billing native-quote
  step dry-run ok "Preview a hAlpha top-up" \
    "see the requests a hAlpha top-up sends; it spends hAlpha for good" \
    -- hipc --output text billing top-up --method halpha --alpha 1 --dry-run
  logout
  end_scenario
}

scenario_drive() {
  begin_scenario drive "read the Drive plans and subscription; plan changes are dry runs"
  login
  step read net "List the Drive plans" "compare the storage plans and prices" \
    -- hipc --output text drive plans
  step read net "Read the Drive subscription" "see the plan in use and its period" \
    -- hipc --output text drive subscription get
  step read net "Read the subscription history" "see past plan changes" \
    -- hipc --output text drive subscription history --limit 5
  # A well-formed intent id that names nothing: the read is a GET, and its refusal is the
  # answer an expired or unknown checkout gets.
  step read net/not_found,permission,owner_only "Read a card checkout" \
    "see how an unknown checkout is answered; a real one is the intent a card subscribe prints" \
    -- hipc --json drive subscription checkout "$MISSING_UUID"
  step dry-run ok "Preview a subscribe" "see what subscribing to Starter sends; it is paid" \
    -- hipc --output text drive subscription subscribe --plan solo --dry-run
  step dry-run ok "Preview a plan change" "see what moving to Plus sends; it is paid" \
    -- hipc --output text drive subscription change --plan duo --dry-run
  step dry-run ok "Preview a cancel" "see what cancelling sends; the period is not refunded" \
    -- hipc --output text drive subscription cancel --dry-run
  [[ $SPENDS == 0 ]] || drive_spend
  logout
  end_scenario
}

# The spend tier's Drive purchase: Starter, the cheapest plan, monthly, from the balance,
# then a cancel. An account that already has a paid plan is left alone: a subscribe would
# change the plan it pays for. A plan that exists is never taken for one this run bought:
# the cancel runs only after this run's own subscribe exited 0, here and in the exit trap.
drive_spend() {
  local subscribed
  SPEND_OK=0
  # An account with no Drive subscription at all answers not_found: a free state too.
  step read net/not_found "Read the Drive subscription as JSON" \
    "see whether a paid plan is in use" -- hipc --json drive subscription get
  if [[ $LIST == 0 && $LAST_EXIT != 0 && "$(error_code "$LAST_ERR")" != not_found ]]; then
    # Without the answer there is no telling a paid plan from none, so nothing is asked.
    BLOCK_SKIP="the Drive subscription could not be read, so nothing is bought"
    record SKIP "spend: a Drive Starter plan"
  elif [[ $LIST == 0 && $LAST_EXIT == 0 ]] &&
    ! jq -e "$DRIVE_CANCELLED"' drive_free' "$LAST_OUT" >/dev/null 2>&1; then
    # Only a free state the script recognises is bought over; any other shape may be a
    # plan the account pays for, and a subscribe would change it.
    BLOCK_SKIP="the Drive subscription is not a free state this script recognises"
    say "drive: nothing is bought; the subscription read answered" \
      "$(jq -c . "$LAST_OUT" 2>/dev/null || echo '(not JSON)')"
    record SKIP "spend: a Drive Starter plan"
  fi
  [[ -n "$BLOCK_SKIP" ]] ||
    spend_gate "a Drive Starter plan, monthly, cancelled at once" \
      "one month of Starter (hipc drive plans shows it), from the balance; a cancel refunds none"
  ledger_add drive-sub solo
  step spend net "Subscribe to Starter" "buy the cheapest plan from the balance" \
    -- hipc --output text drive subscription subscribe --plan solo --period monthly --yes
  subscribed=$LAST_EXIT
  ledger_created drive-sub solo "$subscribed" -
  bought "$subscribed"
  [[ $LIST == 1 || -n "$BLOCK_SKIP" || $subscribed != 0 ]] || printf 'drive-sub\n' >>"$WORK/spent"
  step read net "Read the Drive subscription" "confirm the plan is Starter" \
    -- hipc --json drive subscription get
  [[ $LIST == 1 || $subscribed == 0 ]] ||
    SKIP_REASON="the subscribe did not exit 0, so this run bought nothing to cancel"
  # A plan bought a moment ago can still be confirming; its cancel answers 425 until then.
  RETRY_425=6
  step undo net "Cancel the plan" "undo the subscribe: it ends with the paid period" \
    -- hipc --json drive subscription cancel --yes
  local cancelled=$LAST_EXIT
  # A cancel that exited 0 is not sent again: the exit trap only reads a `cancelled` line.
  [[ $LIST == 1 || -n "$BLOCK_SKIP" || $cancelled != 0 ]] || ledger_mark drive-sub solo cancelled
  step read net "Read the Drive subscription" "confirm the plan is cancelled" \
    -- hipc --json drive subscription get
  check online "the plan is cancelled" -- undone drive-sub "$LAST_OUT"
  if [[ $LIST == 0 && $OFFLINE == 0 && $cancelled == 0 ]] && undone drive-sub "$LAST_OUT"; then
    ledger_remove drive-sub solo
  fi
  block_end
}

# The write tier's bucket policy, as `s3 buckets acl get` reads it back: exactly the one
# statement the run sent, a Deny of s3:GetObject on the prefix it never writes. An Allow, a
# wider action, or another resource under the same Sid is not it.
# shellcheck disable=SC2016 # jq source.
POLICY_IS_DENY='(.policy | if type == "string" then fromjson else . end) as $p
  | ($p.Statement | type) == "array" and ($p.Statement | length) == 1 and
    ($p.Statement[0] | .Sid == "HipcManualDenyNothing" and .Effect == "Deny" and
      ([.Action] | flatten) == ["s3:GetObject"] and
      ((.Resource | type) == "string" and (.Resource | endswith("/hipc-manual-denied/*"))))
'

scenario_s3() {
  local bucket="$1" token_name="$2" token_id fake first_bucket
  begin_scenario s3 "buckets, tokens and an AWS profile; the write tier creates and deletes"
  login
  step read net "List the S3 plans" "compare the S3 plans" -- hipc --output text s3 plans
  step read net "Read the S3 subscription" "see the plan in use, or pay as you go" \
    -- hipc --output text s3 subscription get
  step read net "List the buckets" "see the account's buckets" -- hipc --output text s3 buckets list
  step read net "List the buckets as JSON" "take a bucket name to list its objects" \
    -- hipc --json s3 buckets list
  first_bucket="$(pick "<first-bucket>" "$bucket" '.items[0] | row_name')"
  [[ -n "$first_bucket" ]] || SKIP_REASON="the account has no bucket"
  step read net "List a bucket's objects" "see its first keys; the listing moves no bytes" \
    -- hipc --output text s3 buckets objects list "$first_bucket" --limit 10
  step read net "List the bucket ACLs" "see which buckets are public" \
    -- hipc --output text s3 buckets acl list
  step read net "List the S3 tokens" "see the access keys that exist" \
    -- hipc --output text s3 tokens list
  step read net "Read the S3 subscription history" "see past plan changes" \
    -- hipc --output text s3 subscription history --limit 5
  step read net/not_found,permission,owner_only "Read a card checkout" \
    "see how an unknown checkout is answered; a real one is the intent a card subscribe prints" \
    -- hipc --json s3 subscription checkout "$MISSING_UUID"
  step dry-run ok "Preview a subscribe" "see what subscribing to Pro sends; it is paid" \
    -- hipc --output text s3 subscription subscribe --plan pro --dry-run
  step dry-run ok "Preview a plan change" "see what moving to Business sends; it is paid" \
    -- hipc --output text s3 subscription change --plan business --yes --dry-run
  step dry-run ok "Preview a cancel" "see what cancelling sends; the period is not refunded" \
    -- hipc --output text s3 subscription cancel --yes --dry-run
  # A policy with no statement is refused (MalformedPolicy), so the one the write tier sets
  # holds one statement that grants nothing: it denies everyone reading a prefix the run
  # never writes, so the bucket stays private and nothing the run does is blocked.
  # shellcheck disable=SC2016 # jq source: $b is a jq variable.
  [[ $LIST == 1 ]] || jq -n --arg b "$bucket" '{Version: "2012-10-17", Statement: [{
      Sid: "HipcManualDenyNothing", Effect: "Deny", Principal: "*", Action: "s3:GetObject",
      Resource: "arn:aws:s3:::\($b)/hipc-manual-denied/*"}]}' >"$H/policy.json"
  if [[ "$TIER" == read ]]; then
    step dry-run ok "Preview creating a bucket" "see the request; the read tier creates nothing" \
      -- hipc --output text s3 buckets create "$bucket" --dry-run
    step dry-run ok "Preview replacing a bucket policy" \
      "see the request; the read tier changes no bucket" \
      -- hipc --output text s3 buckets acl update "$bucket" --policy-file "$H/policy.json" \
      --public --remove-others --yes --dry-run
    step dry-run ok "Preview a presigned URL" "see the request; the URL would read the object" \
      -- hipc --output text s3 buckets objects presign "$bucket" hipc-manual.txt --reveal --dry-run
    step dry-run ok "Preview creating a sub token" "see the request for a key scoped to it" \
      -- hipc --output text s3 tokens create --kind sub --name "$token_name" --bucket "$bucket" \
      --dry-run
    step dry-run ok "Preview rotating a sub token" "see the request; nothing is rotated" \
      -- hipc --output text s3 tokens rotate --kind sub "$MISSING_UUID" --reveal --yes --dry-run
  else
    ledger_add bucket "$bucket"
    step write net "Create a bucket" "a new bucket for this test, with a unique name" \
      -- hipc --output text s3 buckets create "$bucket" --yes
    local bucket_created=$LAST_EXIT
    # A bucket's id is its name.
    ledger_created bucket "$bucket" "$bucket_created" "$bucket"
    step read net "List the buckets" "confirm the new bucket is there" \
      -- hipc --json s3 buckets list
    check online "the list shows the new bucket" -- listed "$bucket"
    step read net "Read the bucket's ACL" "see that a new bucket is private" \
      -- hipc --output text s3 buckets acl get "$bucket"
    step write net "Replace the bucket's policy" \
      "set a policy that only denies a prefix the run never writes; the bucket stays private" \
      -- hipc --output text s3 buckets acl update "$bucket" --policy-file "$H/policy.json" \
      --public --remove-others --yes
    step read net "Read the bucket's ACL as JSON" "confirm the new policy is the one sent" \
      -- hipc --json s3 buckets acl get "$bucket"
    check online "the policy is the deny statement" -- last_is "$POLICY_IS_DENY"
    step read net "Presign a download URL" \
      "a URL for a key in the new, empty bucket; the URL is masked in the log" \
      -- hipc --json s3 buckets objects presign "$bucket" hipc-manual.txt --expires-in 60 --reveal
    ledger_add s3-token "$token_name"
    OUT_TO="$H/s3-token.json"
    step write net "Create a sub token for the bucket" \
      "an access key that reaches only this bucket; the secret is shown once, so it goes to a file" \
      -- hipc --json s3 tokens create --kind sub --name "$token_name" --bucket "$bucket" \
      --reveal --yes
    local token_created=$LAST_EXIT
    token_id="$(pick "<token-id>" "$MISSING_UUID" '.id')"
    ledger_created s3-token "$token_name" "$token_created" "$token_id"
    if [[ $OFFLINE == 1 ]]; then
      # The create failed against the dead origin; a stand-in document lets the configure
      # step run, and its secret proves the mask works.
      fake="hipc-manual-test-offline-s3-secret"
      add_secret "$fake" "<redacted>"
      printf '{"kind":"sub","access_key_id":"hip_OFFLINEMANUALTEST","secret":"%s"}\n' "$fake" \
        >"$H/s3-token.json"
      say "offline: ~/s3-token.json holds a stand-in key pair"
    fi
    step read net "List the S3 tokens" "confirm the new token is there" \
      -- hipc --json s3 tokens list
    check online "the list shows the new token" -- listed "$token_name"
    step local ok "Write an AWS profile from the token" \
      "point the AWS CLI at Hippius S3 with the new key pair, read from the file" \
      -- hipc --output text s3 configure --from-token-file "$H/s3-token.json" \
      --aws-profile hipc-manual
    if [[ $OFFLINE == 1 ]]; then
      SKIP_REASON="offline: the AWS CLI would reach s3.hippius.com"
    elif ! command -v aws >/dev/null; then
      SKIP_REASON="the aws CLI is not installed"
    fi
    step ext ok "List the bucket with the AWS CLI" "prove the profile and the key work" \
      -- aws --profile hipc-manual s3 ls "s3://$bucket"
    undo_needs "$token_created" "$token_id"
    step write net "Rotate the sub token" \
      "a new secret for the same key; the old one stops working, and the new one is masked" \
      -- hipc --json s3 tokens rotate --kind sub "$token_id" --reveal --yes
    # A rotation may answer a new id for the key; the revoke and the ledger follow it.
    local rotated
    rotated="$(pick "$token_id" "$token_id" '.id')"
    if [[ $LIST == 0 && $LAST_EXIT == 0 && -n "$rotated" && "$rotated" != "$token_id" ]]; then
      token_id="$rotated"
      ledger_rename s3-token "$token_name" "$token_name" "$token_id"
    fi
    undo_needs "$token_created" "$token_id"
    step undo net "Revoke the sub token" "undo the create: the key stops working" \
      -- hipc --output text s3 tokens revoke --kind sub "$token_id" --yes
    local revoked=$LAST_EXIT
    step read net "List the S3 tokens" "confirm the token is gone or revoked" \
      -- hipc --json s3 tokens list
    check online "the list no longer shows the token as live" -- not_listed "$token_name"
    ledger_settle s3-token "$token_name" "$revoked"
    undo_needs "$bucket_created" "$bucket"
    step undo net "Delete the bucket" "undo the create" \
      -- hipc --output text s3 buckets delete "$bucket" --yes
    local deleted=$LAST_EXIT
    step read net "List the buckets" "confirm the bucket is gone" -- hipc --json s3 buckets list
    check online "the list no longer shows the bucket" -- not_listed "$bucket"
    ledger_settle bucket "$bucket" "$deleted"
  fi
  logout
  end_scenario
}

scenario_hub() {
  local key_name="$1" repository key_id subscription reference namespace model
  # A name no account is likely to hold: the availability read answers for it.
  namespace="hipc-manual-${key_name##*-}"
  [[ $LIST == 0 ]] || namespace="<namespace>"
  begin_scenario hub "read the registry; the write tier creates and deletes a CI key"
  login
  step read net "List the Hub plans" "compare the registry plans" -- hipc --output text hub plans
  step read net "Read the Hub subscription" "see the plan in use" \
    -- hipc --json hub subscription get
  subscription="$(pick "<subscription>" "$MISSING_NUMBER" '.subscription_id // .id')"
  [[ -n "$subscription" ]] || subscription="$MISSING_NUMBER"
  step dry-run ok "Preview a cancel" "see what cancelling sends; the period is not refunded" \
    -- hipc --output text hub subscription cancel "$subscription" --yes --dry-run
  step read net "Read the namespace status" "see whether the namespace is provisioned" \
    -- hipc --output text hub namespace status
  step read net "Check a namespace name" "see whether a new name is free to claim" \
    -- hipc --output text hub namespace availability "$namespace"
  step dry-run ok "Preview claiming a namespace" \
    "see the request; a namespace is the account's one name in the registry, so none is claimed" \
    -- hipc --output text hub namespace create "$namespace" --reveal --yes --dry-run
  step read net/not_found,hub_unprovisioned "Read the namespace" "see the namespace's name and limits" \
    -- hipc --json hub namespace get
  step read net/not_found,hub_unprovisioned "Read the robot login" "see the login a pipeline pushes with" \
    -- hipc --json hub namespace robot get
  # Both change what every pipeline and every puller of the namespace sees, so they are dry
  # runs in every tier.
  step dry-run ok "Preview making the namespace private" \
    "see the request; who may pull would change for everyone" \
    -- hipc --output text hub namespace update --visibility private --yes --dry-run
  step dry-run ok "Preview rotating the robot secret" \
    "see the request; every docker login with the old secret would stop working" \
    -- hipc --output text hub namespace robot rotate --reveal --yes --dry-run
  step read net/not_found,hub_unprovisioned "List the registry events" "see the namespace's audit trail" \
    -- hipc --json hub events list
  step read net/not_found,hub_unprovisioned "Read the usage" "see storage and pulls against the plan" \
    -- hipc --json hub usage
  step read net/hub_unprovisioned "List the repositories" "see the images in the namespace" \
    -- hipc --json hub repositories list
  repository="$(pick "<repository>" "library/hipc-manual" 'first(rows // [] | .[] | .name)')"
  block_needs "$repository" "the namespace has no repository"
  step read net "Read a repository" "see the first repository's size and pulls" \
    -- hipc --output text hub repositories get "$repository"
  step read net "List a repository's artifacts" "see the tags and digests of the first one" \
    -- hipc --json hub artifacts list "$repository"
  reference="$(pick "<reference>" "latest" \
    'first(rows // [] | .[] | (.digest // (.tags // [] | .[0] | .name? // .)))')"
  [[ -n "$reference" ]] || SKIP_REASON="the repository has no artifact"
  step read net "Read an artifact" "see the first artifact's digest, tags and size" \
    -- hipc --output text hub artifacts get "$repository" "$reference"
  step dry-run ok "Preview deleting the artifact" "see the request; nothing is deleted" \
    -- hipc --output text hub artifacts delete "$repository" "${reference:-latest}" --yes --dry-run
  step dry-run ok "Preview deleting the repository" "see the request; nothing is deleted" \
    -- hipc --output text hub repositories delete "$repository" --yes --dry-run
  block_end
  # JSON, so a 403 for an account with no namespace can be told apart.
  step read net/hub_unprovisioned "List the Hub keys" "see the CI keys that exist" \
    -- hipc --json hub keys list
  step read net "List the models" "see the model index, first page" \
    -- hipc --json hub models list --limit 5
  model="$(pick "<model>" "library/hipc-manual" 'first(rows // [] | .[] | .name // .model)')"
  [[ -n "$model" ]] || SKIP_REASON="the index has no model"
  step read net "Read a model" "see the first model's versions" \
    -- hipc --output text hub models get "${model:-library/hipc-manual}" --limit 5
  step read net "List the model formats" "see the formats the index knows" \
    -- hipc --output text hub models formats
  step dry-run ok "Preview a subscribe" "see what subscribing to Builder sends; it is paid" \
    -- hipc --output text hub subscription subscribe --plan Builder --dry-run
  if [[ "$TIER" == read ]]; then
    step dry-run ok "Preview creating a CI key" "see the request; the read tier creates nothing" \
      -- hipc --output text hub keys create --name "$key_name" --role read --expires-in 1 \
      --dry-run
    step dry-run ok "Preview rotating a CI key" "see the request; the read tier rotates nothing" \
      -- hipc --output text hub keys rotate "$MISSING_NUMBER" --reveal --yes --dry-run
  else
    ledger_add hub-key "$key_name"
    OUT_TO="$H/hub-key.json"
    step write net "Create a read-only CI key" \
      "a pull-only key that expires in a day; the secret is shown once, so it goes to a file" \
      -- hipc --json hub keys create --name "$key_name" --role read --expires-in 1 --reveal --yes
    local created=$LAST_EXIT
    key_id="$(pick "<hub-key-id>" "$MISSING_NUMBER" '.id')"
    ledger_created hub-key "$key_name" "$created" "$key_id"
    step read net "List the Hub keys" "confirm the new key is there" -- hipc --json hub keys list
    check online "the list shows the new key" -- listed "$key_name"
    undo_needs "$created" "$key_id"
    step read net "Read the CI key" "see the new key; no secret is shown" \
      -- hipc --output text hub keys get "$key_id"
    hub_docker_login
    undo_needs "$created" "$key_id"
    step write net "Rotate the CI key" \
      "a new secret for the same key; the old one stops working, and the new one is masked" \
      -- hipc --json hub keys rotate "$key_id" --reveal --yes
    undo_needs "$created" "$key_id"
    step undo net "Delete the CI key" "undo the create" \
      -- hipc --output text hub keys delete "$key_id" --yes
    local deleted=$LAST_EXIT
    step read net "List the Hub keys" "confirm the key is gone" -- hipc --json hub keys list
    check online "the list no longer shows the key" -- not_listed "$key_name"
    ledger_settle hub-key "$key_name" "$deleted"
  fi
  logout
  end_scenario
}

# `docker login` with the new key, then `docker logout`, in the scenario's DOCKER_CONFIG.
# Only with --docker: it needs a running Docker daemon.
hub_docker_login() {
  local login=""
  if [[ $LIST == 0 ]]; then
    if [[ $OFFLINE == 1 ]]; then
      SKIP_REASON="offline: docker would reach registry.hippius.com"
    elif [[ $DOCKER == 0 ]]; then
      SKIP_REASON="pass --docker to run it"
    elif ! command -v docker >/dev/null; then
      SKIP_REASON="docker is not installed"
    else
      login="$(jq -r '.login // empty' "$H/hub-key.json" 2>/dev/null || true)"
      jq -r '.secret // empty' "$H/hub-key.json" >"$H/hub-key.secret" 2>/dev/null || true
      [[ -n "$login" && -s "$H/hub-key.secret" ]] ||
        SKIP_REASON="the create answer has no login and secret"
    fi
  else
    login="<login>"
  fi
  local reason="$SKIP_REASON"
  IN_FILE="$H/hub-key.secret"
  IN_SHOW="< (the key's secret, on stdin)"
  step ext ok "Log Docker in with the key" "what a pipeline does before it pulls" \
    -- docker login registry.hippius.com -u "$login" --password-stdin
  SKIP_REASON="$reason"
  step ext ok "Log Docker out" "drop the stored registry login" \
    -- docker logout registry.hippius.com
}

scenario_compute() {
  local key_name="$1" vm_name="$2" db_name="$3" flavor key_id vm db
  begin_scenario compute \
    "prices and capacity, VMs, databases, runners and SSH keys; a launch is a dry run below spend"
  login
  step read net "List the regions" "see where a VM can run" -- hipc --output text compute regions
  step read net "List the flavors" "see the VM sizes" -- hipc --json compute flavors
  # The smallest flavor that is available: the fewest vCPUs, then the least memory.
  flavor="$(pick "<flavor>" "small" '[.. | objects | select(has("vcpus") and has("code"))
    | select(.available != false)] | sort_by(.vcpus, (.memory_mb // .memory_gb // 0))
    | first | .code')"
  step read net "Read the account's prices" "see what a VM costs this account" \
    -- hipc --output text compute prices
  step read net "Read the public prices" "see the list prices" \
    -- hipc --output text compute public-prices
  step read net "Read the quota" "see how much compute the account may use" \
    -- hipc --output text compute quota
  step read net "Read the charge cap" "see the most compute may spend before launches stop" \
    -- hipc --output text compute charge-cap
  step read net "List the images" "see the operating systems a VM can boot" \
    -- hipc --output text compute images
  step read net "List the apps" "see the one-click apps a VM can start with" \
    -- hipc --output text compute apps
  step read net "Read the placement groups" "see whether anti-affinity groups are offered" \
    -- hipc --output text compute placement-groups
  step read net "Read the backup options" "see the backup intervals and what they cost" \
    -- hipc --output text compute backups options
  [[ -n "$flavor" ]] || SKIP_REASON="the flavor list is empty"
  step read net "Check a launch" "ask whether the smallest flavor can launch now" \
    -- hipc --output text compute feasibility --flavor "$flavor"
  step read net "List the VMs" "see the running VMs" -- hipc --output text compute vms list
  step read net "List the SSH keys" "see every key a VM can get, all pages" \
    -- hipc --output text compute ssh-keys list --all
  step read net "List the account SSH keys" "see the keys every VM gets" \
    -- hipc --output text compute account-ssh-keys
  step read net "Check public IP availability" "see whether a public IPv4 is free" \
    -- hipc --output text compute public-ip availability
  [[ $LIST == 1 ]] || command -v ssh-keygen >/dev/null || SKIP_REASON="ssh-keygen is missing"
  step ext ok "Make an SSH key pair" "a throwaway key for this test, in the temporary home" \
    -- ssh-keygen -q -t ed25519 -N "" -C hipc-manual -f "$H/hipc-manual-key"
  [[ -n "$flavor" ]] || SKIP_REASON="the flavor list is empty"
  step dry-run ok "Preview a VM launch" "see the launch request; a VM spends credits" \
    -- hipc --output text compute vms create --flavor "$flavor" --name hipc-manual \
    --ssh-public-key-file "$H/hipc-manual-key.pub" --dry-run
  step dry-run ok "Preview a public IP" "see the request; a public IP spends credits" \
    -- hipc --output text compute public-ip create vm_example --dry-run
  if [[ "$TIER" == read ]]; then
    step dry-run ok "Preview adding the SSH key" "see the request; the read tier creates nothing" \
      -- hipc --output text compute ssh-keys create --name "$key_name" \
      --public-key-file "$H/hipc-manual-key.pub" --dry-run
  else
    ledger_add ssh-key "$key_name"
    step write net "Add the SSH key" "store the public key for later launches" \
      -- hipc --json compute ssh-keys create --name "$key_name" \
      --public-key-file "$H/hipc-manual-key.pub" --yes
    local created=$LAST_EXIT
    key_id="$(pick "<ssh-key-id>" "$MISSING_NUMBER" '.id')"
    ledger_created ssh-key "$key_name" "$created" "$key_id"
    step read net "List the SSH keys" "confirm the new key is there" \
      -- hipc --json compute ssh-keys list --all
    check online "the list shows the new key" -- listed "$key_name"
    undo_needs "$created" "$key_id"
    step read net "Read the SSH key" "see the new key's fingerprint" \
      -- hipc --output text compute ssh-keys get "$key_id"
    undo_needs "$created" "$key_id"
    step undo net "Delete the SSH key" "undo the create" \
      -- hipc --output text compute ssh-keys delete "$key_id" --yes
    local deleted=$LAST_EXIT
    step read net "List the SSH keys" "confirm the key is gone" \
      -- hipc --json compute ssh-keys list --all
    check online "the list no longer shows the key" -- not_listed "$key_name"
    ledger_settle ssh-key "$key_name" "$deleted"
  fi
  step read net "List the VMs as JSON" "take the first VM to read" \
    -- hipc --json compute vms list
  vm="$(pick "<vm>" "$MISSING_UUID" 'first(rows // [] | .[] | objects | select(revoked | not)
    | .vm_id // .id)')"
  # A VM or database being deleted (decommissioning) is skipped; see `revoked`.
  local first_vm="$vm" first_db
  [[ $SPENDS == 0 ]] || vm_spend "$flavor" "$vm_name"
  # Without a purchase, the reads run on the first VM the account has, as in the read tier.
  if [[ $SPENDS == 0 || $SPEND_OK == 0 ]]; then
    vm="$first_vm"
    block_needs "$vm" "the account has no VM"
    vm_reads "$vm"
    block_end
  fi
  vm_dry_runs "${vm:-$MISSING_UUID}" "${flavor:-small}"
  step read net "List the databases" "see the managed PostgreSQL databases" \
    -- hipc --json compute databases list
  db="$(pick "<database>" "$MISSING_NUMBER" 'first(rows // [] | .[] | objects
    | select(revoked | not) | .id)')"
  first_db="$db"
  step read net "List the refused database launches" "see why a launch left no database" \
    -- hipc --output text compute databases refusals
  [[ $SPENDS == 0 ]] || database_spend "$db_name"
  if [[ $SPENDS == 0 || $SPEND_OK == 0 ]]; then
    db="$first_db"
    block_needs "$db" "the account has no database"
    database_reads "$db"
    block_end
  fi
  database_dry_runs "${db:-$MISSING_NUMBER}"
  runners
  app_engine
  logout
  end_scenario
}

# The reads of one VM. In the spend tier the VM is the one it launched.
vm_reads() {
  local vm="$1"
  step read net "Read the VM" "see its state, flavor and addresses" \
    -- hipc --output text compute vms get "$vm"
  # A VM that is not confidential has no attestation; a VM that never failed over has no
  # history, and one with no backup policy no policy: each may answer not_found.
  step read net/not_found "Read the VM's attestation" "see the evidence of what it runs" \
    -- hipc --json compute vms attestation "$vm"
  step read net/not_found "List the VM's failovers" "see whether it moved to another server" \
    -- hipc --json compute vms failovers "$vm"
  step read net "List the resize options" "see the flavors the VM can move to" \
    -- hipc --output text compute vms resize-options "$vm"
  step read net/not_found "Read a resize" "see how an unknown resize id is answered" \
    -- hipc --json compute vms resize-status "$vm" "$MISSING_NUMBER"
  step read net "Read the power policy" "see what a power-off from inside the VM does" \
    -- hipc --output text compute vms power-policy get "$vm"
  step read net/not_found "Read the public IP usage" "see the traffic of the VM's address" \
    -- hipc --json compute public-ip usage "$vm" --range 24h
  step read net "Read the firewall" "see the VM's inbound rules" \
    -- hipc --output text compute firewall get "$vm"
  step read net "List the exposures" "see the guest ports the internet can reach" \
    -- hipc --output text compute exposures list "$vm"
  step read net/not_found "Read the backup policy" "see how often the VM is backed up" \
    -- hipc --json compute backups policy get "$vm"
  step read net/not_found "List the backups" "see the restore points" \
    -- hipc --json compute backups list "$vm"
  step read net/not_found "Read the last restore" "see the progress of a restore, if any" \
    -- hipc --json compute backups restore-status "$vm"
}

# The VM writes that are not undone within the run, or that a running VM would feel, as
# dry runs in every tier.
vm_dry_runs() {
  local vm="$1" flavor="$2"
  [[ $LIST == 1 ]] || printf '{"rules":[]}\n' >"$H/firewall.json"
  step dry-run ok "Preview stopping the VM" "see the request; nothing is stopped" \
    -- hipc --output text compute vms stop "$vm" --yes --dry-run
  step dry-run ok "Preview starting the VM" "see the request" \
    -- hipc --output text compute vms start "$vm" --dry-run
  step dry-run ok "Preview rebooting the VM" "see the request; nothing reboots" \
    -- hipc --output text compute vms reboot "$vm" --yes --dry-run
  step dry-run ok "Preview a resize" "see the request; a bigger flavor costs more" \
    -- hipc --output text compute vms resize "$vm" --flavor "$flavor" --yes --dry-run
  step dry-run ok "Preview a power policy" "see the request" \
    -- hipc --output text compute vms power-policy update "$vm" --on-guest-poweroff stop --dry-run
  step dry-run ok "Preview a firewall change" "see the request; it would close every port" \
    -- hipc --output text compute firewall update "$vm" --rules-file "$H/firewall.json" \
    --remove-others --yes --dry-run
  step dry-run ok "Preview an exposure" "see the request; it would open a guest port" \
    -- hipc --output text compute exposures create "$vm" --port 8080 --mode http --yes --dry-run
  step dry-run ok "Preview removing an exposure" "see the request" \
    -- hipc --output text compute exposures delete "$vm" "$MISSING_NUMBER" --yes --dry-run
  step dry-run ok "Preview a backup policy" "see the request; backups are billed" \
    -- hipc --output text compute backups policy update "$vm" --interval 86400 --yes --dry-run
  step dry-run ok "Preview removing the backup policy" "see the request" \
    -- hipc --output text compute backups policy delete "$vm" --yes --dry-run
  step dry-run ok "Preview a restore" "see the request; a restore rolls the disk back" \
    -- hipc --output text compute backups restore "$vm" --run-id "$MISSING_UUID" \
    --accept-rollback --yes --dry-run
  step dry-run ok "Preview releasing the public IP" "see the request" \
    -- hipc --output text compute public-ip delete "$vm" --yes --dry-run
  step dry-run ok "Preview deleting the VM" "see the request; nothing is deleted" \
    -- hipc --output text compute vms delete "$vm" --yes --dry-run
}

# The spend tier's VM: the smallest flavor, launched with --wait, then a power policy, an
# exposure and a backup policy, each undone, a public IP, released, and the VM deleted. The
# VM, then its IP, are in the ledger before their creates; deleting the VM removes the
# exposure and the policy with it, so those two are undone as steps only. The exposure is of
# a port nothing listens on, never SSH, and the public IP is attached only once the exposure
# is removed and the firewall holds no rule (every inbound port closed), so the VM is never
# reachable from the internet through either.
vm_spend() {
  local flavor="$1" vm_name="$2" created ip_created closed=1 walled=1 outer
  SPEND_OK=0
  [[ -n "$flavor" ]] || BLOCK_SKIP="the flavor list is empty"
  [[ $LIST == 1 || -s "$H/hipc-manual-key.pub" ]] || BLOCK_SKIP="the SSH key pair was not made"
  [[ -n "$BLOCK_SKIP" ]] ||
    spend_gate "a VM of the smallest flavor ($flavor) with a public IP, deleted at the end" \
      "the flavor's and the IP's hourly price (hipc compute prices) for the minutes they run"
  ledger_add vm "$vm_name"
  step spend net "Launch a VM" "the smallest flavor, waiting until it runs" \
    -- hipc --json compute vms create --flavor "$flavor" --name "$vm_name" \
    --ssh-public-key-file "$H/hipc-manual-key.pub" --wait --yes
  created=$LAST_EXIT
  vm="$(pick "<new-vm>" "$MISSING_UUID" '.vm_id // .id')"
  ledger_created vm "$vm_name" "$created" "$vm"
  # A wait that ends early still prints the VM, so its id goes in the ledger either way.
  if [[ -z "$BLOCK_SKIP" ]]; then
    ledger_answered vm "$vm_name" "$(answered '.vm_id // .id')"
    bought "$created" "$(answered '.vm_id // .id')"
    # A VM launch can set up the account's private network (compare_snapshots).
    [[ $LIST == 1 || ($created != 0 && -z "$(answered '.vm_id // .id')") ]] ||
      printf 'vm\n' >>"$WORK/spent"
  fi
  if [[ $LIST == 0 && $OFFLINE == 0 && -z "$BLOCK_SKIP" && ($created != 0 || -z "$vm") ]]; then
    BLOCK_SKIP="the launch did not exit 0; the exit trap deletes the VM by its id, or by name"
  fi
  vm_reads "$vm"
  step write net "Set the power policy" "a power-off from inside the VM now stops it" \
    -- hipc --output text compute vms power-policy update "$vm" --on-guest-poweroff stop
  step read net "Read the power policy" "confirm it is stop" \
    -- hipc --json compute vms power-policy get "$vm"
  check online "the policy is stop" -- last_is '.on_guest_poweroff == "stop"'
  step write net "Expose a port" "make guest port 8080, where nothing listens, reachable" \
    -- hipc --json compute exposures create "$vm" --port 8080 --mode http --yes
  local exposure
  exposure="$(pick "<exposure>" "$MISSING_NUMBER" '.id // .exposure_id')"
  [[ -n "$exposure" ]] || SKIP_REASON="the exposure create answered no id"
  step undo net "Remove the exposure" "undo it: the port is private again" \
    -- hipc --output text compute exposures delete "$vm" "${exposure:-$MISSING_NUMBER}" --yes
  [[ "$LAST_EXIT" != 0 ]] || closed=0
  step write net "Set a daily backup policy" "the VM is backed up once a day" \
    -- hipc --output text compute backups policy update "$vm" --interval 86400 --yes
  step undo net "Remove the backup policy" "undo it before the first backup is due" \
    -- hipc --output text compute backups policy delete "$vm" --yes
  step write net "Reboot the VM" "a reboot keeps the VM and its disk" \
    -- hipc --output text compute vms reboot "$vm" --yes
  # An empty rule list blocks every inbound port of the public IP, before there is one; the
  # VM delete removes the firewall with the VM.
  [[ $LIST == 1 ]] || printf '{"rules":[]}\n' >"$H/firewall.json"
  step write net "Close every inbound port" \
    "an empty firewall blocks all inbound traffic before the public IP is attached" \
    -- hipc --output text compute firewall update "$vm" --rules-file "$H/firewall.json" \
    --remove-others --yes
  [[ "$LAST_EXIT" != 0 ]] || walled=0
  step read net "Read the firewall" "confirm it has no rule" \
    -- hipc --json compute firewall get "$vm"
  check online "the firewall has no rule" -- last_is '(.rules // []) | length == 0'
  # The IP steps need the exposure gone and the firewall closed, offline too.
  outer="$BLOCK_SKIP"
  if [[ $LIST == 0 && $closed != 0 && -z "$BLOCK_SKIP" ]]; then
    BLOCK_SKIP="the exposure was not removed, so no public IP is attached"
  elif [[ $LIST == 0 && $walled != 0 && -z "$BLOCK_SKIP" ]]; then
    BLOCK_SKIP="the firewall was not closed, so no public IP is attached"
  fi
  ledger_add public-ip "$vm_name"
  step spend net "Attach a public IP" "an IPv4 address, billed per second" \
    -- hipc --json compute public-ip create "$vm" --yes
  ip_created=$LAST_EXIT
  ledger_created public-ip "$vm_name" "$ip_created" "$vm"
  [[ -n "$BLOCK_SKIP" ]] || bought "$ip_created"
  step read net "Read the public IP" "see the new address" \
    -- hipc --json compute public-ip get "$vm"
  step undo net "Release the public IP" "undo the attach: billing stops" \
    -- hipc --output text compute public-ip delete "$vm" --yes
  local released=$LAST_EXIT
  step read net/not_found "Read the public IP" "confirm the address is gone" \
    -- hipc --json compute public-ip get "$vm"
  if [[ $LIST == 0 && $OFFLINE == 0 && $released == 0 ]] && undone public-ip "$LAST_OUT"; then
    ledger_remove public-ip "$vm_name"
  fi
  BLOCK_SKIP="$outer"
  step undo net "Delete the VM" "undo the launch: billing stops" \
    -- hipc --output text compute vms delete "$vm" --yes
  local deleted=$LAST_EXIT
  step read net "List the VMs" "confirm the VM is gone" -- hipc --json compute vms list
  check online "the list no longer shows the VM" -- not_listed "$vm_name"
  [[ -n "$BLOCK_SKIP" ]] || ledger_settle vm "$vm_name" "$deleted"
  block_end
}

database_reads() {
  local db="$1"
  step read net "Read the database" "see its state and how to connect" \
    -- hipc --output text compute databases get "$db"
  step read net "List the database's events" "see what happened to it" \
    -- hipc --output text compute databases events "$db"
  step read net "Read the database's metrics" "see its CPU, memory and storage" \
    -- hipc --output text compute databases metrics "$db"
}

database_dry_runs() {
  local db="$1"
  step dry-run ok "Preview a database" "see the request; a database is billed by usage" \
    -- hipc --output text compute databases create --tier starter --size s --engine-version 17 \
    --reveal --yes --dry-run
  step dry-run ok "Preview a restart" "see the request; nothing restarts" \
    -- hipc --output text compute databases restart "$db" --yes --dry-run
  step dry-run ok "Preview a restore" "see the request; a restore makes a new database" \
    -- hipc --output text compute databases restore "$db" --reveal --yes --dry-run
  step dry-run ok "Preview exposing the database" "see the request; it would be reachable" \
    -- hipc --output text compute databases exposure create "$db" --yes --dry-run
  step dry-run ok "Preview closing the database" "see the request" \
    -- hipc --output text compute databases exposure delete "$db" --yes --dry-run
  step dry-run ok "Preview deleting the database" "see the request; nothing is deleted" \
    -- hipc --output text compute databases delete "$db" --yes --dry-run
}

# The spend tier's database: the smallest (starter, s), deleted once it has been read. The
# TLS files go to a new directory in the temporary home; the password and the backup
# passphrase stay in the answer, which the mask hides.
database_spend() {
  local db_name="$1" created
  SPEND_OK=0
  spend_gate "a starter database of size s, deleted at the end" \
    "its usage price (hipc compute prices) for the minutes it runs"
  [[ $LIST == 1 ]] || mkdir -p "$H/database"
  ledger_add database "$db_name"
  step spend net "Create a database" "the smallest PostgreSQL database" \
    -- hipc --json compute databases create --tier starter --size s --engine-version 17 \
    --name "$db_name" --credentials-dir "$H/database" --reveal --yes
  created=$LAST_EXIT
  db="$(pick "<new-database>" "$MISSING_NUMBER" '.id')"
  ledger_created database "$db_name" "$created" "$db"
  # A create that fails after the server acted may still print the database's id.
  if [[ -z "$BLOCK_SKIP" ]]; then
    ledger_answered database "$db_name" "$(answered '.id')"
    bought "$created" "$(answered '.id')"
    # The server makes a backup bucket and its writer token for the database; the
    # snapshot diff reports them apart (compare_snapshots).
    [[ $LIST == 1 || -z "$(answered '.id')" ]] ||
      printf 'database %s\n' "$(answered '.id')" >>"$WORK/spent"
  fi
  if [[ $LIST == 0 && $OFFLINE == 0 && -z "$BLOCK_SKIP" && ($created != 0 || -z "$db") ]]; then
    BLOCK_SKIP="the create did not exit 0; the exit trap deletes the database by its id, or by name"
  fi
  wait_for_database "$db"
  database_reads "$db"
  step undo net "Delete the database" "undo the create: billing stops" \
    -- hipc --output text compute databases delete "$db" --yes
  local deleted=$LAST_EXIT
  step read net "List the databases" "confirm the database is gone" \
    -- hipc --json compute databases list
  check online "the list no longer shows the database" -- not_listed "$db_name"
  [[ -n "$BLOCK_SKIP" ]] || ledger_settle database "$db_name" "$deleted"
  block_end
}

# A database being created cannot be deleted yet: wait up to ten minutes for it to leave
# its first state, reading it every 15 seconds outside the transcript.
wait_for_database() {
  local db="$1" i
  [[ $LIST == 0 && $OFFLINE == 0 && -z "$BLOCK_SKIP" ]] || return 0
  say "waiting for database $db to run (up to 10 minutes)"
  for ((i = 0; i < 40; i++)); do
    if "$HIPC" --json compute databases get "$db" >"$WORK/wait.out" 2>/dev/null </dev/null &&
      jq -e '(.status // "") | test("running|failed|error"; "i")' "$WORK/wait.out" >/dev/null; then
      say "database $db: $(jq -r '.status' "$WORK/wait.out")"
      return 0
    fi
    sleep 15
  done
  say "database $db did not run within 10 minutes; the delete may be refused"
}

# App Engine, reads only: whether the account may use it, its apps, and the first app's
# compose, runtime and guest components. The compose checks validate and fix send a small
# compose and launch nothing. Outside the feature the routes answer 403, `permission`.
app_engine() {
  local app
  [[ $LIST == 1 ]] ||
    printf 'services:\n  web:\n    image: nginx:1.27\n    ports:\n      - "8080:80"\n' \
      >"$H/compose.yml"
  step read net "Read App Engine" "see whether the account may use it, and its limits" \
    -- hipc --output text compute app-engine get
  step read net/permission "List the apps" "see the compose apps the account runs" \
    -- hipc --json compute app-engine apps list
  app="$(pick "<app>" "$MISSING_UUID" 'first(rows // [] | .[] | .vm_id // .id)')"
  block_needs "$app" "the account has no App Engine app"
  step read net "Read an app" "see the first app's compose and its last revisions" \
    -- hipc --output text compute app-engine apps get "$app"
  step read net "Read the app's runtime" "see what the app's runner reports" \
    -- hipc --output text compute app-engine apps runtime "$app"
  step read net "Read the guest components" "see the versions inside the app's VM" \
    -- hipc --output text compute app-engine guest-components get "$app"
  step read net "Read the guest component update" "see whether an update is pending or done" \
    -- hipc --output text compute app-engine guest-components update-status "$app"
  block_end
  # A compose the platform refuses exits 1 `validation`, with every issue listed.
  step read net/permission,validation "Validate a compose" \
    "check a one-service compose; nothing is launched" \
    -- hipc --json compute app-engine validate --compose-file "$H/compose.yml"
  step read net/permission,validation "Fix a compose" \
    "see the compose the platform would run, written to a new file" \
    -- hipc --json compute app-engine fix --compose-file "$H/compose.yml" \
    --output-file "$H/compose-fixed.yml"
  # A launch is a billed VM with a public IP, so it is a dry run in every tier. Its secrets
  # file holds this run's stand-in value, which the mask knows from the start: the dry run
  # must show a placeholder for it, and the log scan finds it if it shows anywhere.
  if [[ $LIST == 0 ]]; then
    printf '{"DB_PASSWORD":"%s"}\n' "$APP_SECRET" >"$H/app-secrets.json"
    chmod 600 "$H/app-secrets.json"
  fi
  step dry-run ok "Preview launching an app" \
    "see the validate and the launch requests; the secret is shown as a placeholder" \
    -- hipc --json compute app-engine apps create --flavor small --compose-file "$H/compose.yml" \
    --secrets-file "$H/app-secrets.json" --name hipc-manual --dry-run
  check always "the dry run shows a placeholder for the secret" -- last_is \
    '.requests[-1].body.secrets.DB_PASSWORD == "<the DB_PASSWORD from --secrets-file>"'
  check always "the dry run does not show the secret" -- app_secret_not_in_last
}

app_secret_not_in_last() {
  ! grep -qF -f "$WORK/app-secret" -- "$LAST_OUT" "$LAST_ERR"
}

# GitHub Actions runners: the reads, and the writes as dry runs, but for the write tier's
# concurrency and spend cap changes, each put back (see below); enabling runners bills
# every job a linked repository starts.
runners() {
  step read net "Read the runner settings" "see whether runners are on, and their sizes" \
    -- hipc --json compute runners settings get
  if [[ $WRITES == 0 ]]; then
    step dry-run ok "Preview a runner setting" "see the request; runners bill each job" \
      -- hipc --output text compute runners settings update --max-concurrent 1 --yes --dry-run
  else
    runner_max_change
  fi
  step read net "List the linked GitHub installations" "see the organizations runners serve" \
    -- hipc --json compute runners installations list
  local installation
  installation="$(pick "<installation>" "$MISSING_NUMBER" 'first(rows // [] | .[] | .id)')"
  step dry-run ok "Preview unlinking an installation" "see the request; nothing is unlinked" \
    -- hipc --output text compute runners installations delete "${installation:-$MISSING_NUMBER}" \
    --yes --dry-run
  step dry-run ok "Preview linking GitHub" "see the request; a person installs the App" \
    -- hipc --output text compute runners connect --dry-run
  step read net "List the runner jobs" "see the jobs runners ran" \
    -- hipc --output text compute runners jobs
  step read net "Read the runner usage" "see what the runners cost this month" \
    -- hipc --output text compute runners usage
  # A server without runner spend limits answers not_found ("not enabled on this server").
  step read net/not_found "Read the runner limit" "see the monthly runner spend cap" \
    -- hipc --json compute runners limit get
  if [[ $WRITES == 0 ]]; then
    step dry-run ok "Preview a runner limit" "see the request; it would change the cap" \
      -- hipc --output text compute runners limit update --monthly-cap 1 --yes --dry-run
  else
    runner_cap_change
  fi
}

# The write tier lowers max_concurrent by one, reads it back, and puts it back. Lowering never
# lets runners spend more; at 1 or less a lower value would stop them, so it is skipped. The
# server refuses a max_concurrent above quota_runners, so a stored value above the quota could
# not be put back, and the change is skipped then too, and when the settings show no quota.
# Both are at most 9 digits, so bash arithmetic cannot wrap.
runner_max_change() {
  local max quota lower="" why
  max="$(pick_answered "<max-concurrent>" "2" '.max_concurrent | numbers')"
  quota="$(pick_answered "<quota-runners>" "2" '.quota_runners | numbers')"
  why="max_concurrent is ${max:-not in the settings}, so there is no lower value"
  if [[ $LIST == 1 ]]; then
    lower="<max-concurrent-1>"
  elif ! [[ "$max" =~ ^[0-9]{1,9}$ ]] || ((10#$max <= 1)); then
    :
  elif ! [[ "$quota" =~ ^[0-9]{1,9}$ ]]; then
    why="the settings show no quota_runners number, so the script cannot tell whether $max"
    why+=" could be put back"
  elif ((10#$max > 10#$quota)); then
    why="max_concurrent $max is above quota_runners $quota, a value the server refuses, so"
    why+=" it could not be put back"
  else
    lower=$((10#$max - 1))
  fi
  block_needs "$lower" "$why"
  change_begin runner-max settings "$max"
  step write net "Lower the runner concurrency" "one runner fewer at once; it is put back next" \
    -- hipc --json compute runners settings update --max-concurrent "$lower" --yes
  step read net "Read the runner settings" "confirm the server kept the lower value" \
    -- hipc --json compute runners settings get
  check online "max_concurrent is the lower value" -- shows "$RUNNER_MAX_IS" settings "$lower"
  step undo net "Put the runner concurrency back" "the value read before the change" \
    -- hipc --json compute runners settings update --max-concurrent "$max" --yes
  local restored=$LAST_EXIT
  step read net "Read the runner settings" "confirm it is back" \
    -- hipc --json compute runners settings get
  check online "max_concurrent is back" -- shows "$RUNNER_MAX_IS" settings "$max"
  change_settle runner-max settings "$restored" "$RUNNER_MAX_IS" "$max"
  block_end
}

# The write tier raises an existing runner spend cap by one credit, and puts it back. A raise,
# not a cut: a lower cap could stop a job that runs meanwhile. Only a cap of a whole number of
# credits is raised; no cap at all is left alone, since what a `null` cap means is not
# confirmed. hipc checks that the answer to the raise shows the cap it sent, and a read
# confirms it. The put-back does not go through hipc's own reading of the fields, which is
# what this step tests: it sends the server's own `monthly_cap`, as read before the change,
# back with `hipc api PUT`, so it restores the cap in whatever unit the server keeps it.
runner_cap_change() {
  local cap body higher=""
  cap="$(pick_answered "<cap>" "20" '.monthly_cap_credits')"
  body="$(pick_answered "<body>" '{"monthly_cap":"20000000000000000000"}' \
    'select(has("monthly_cap")) | {monthly_cap} | tojson')"
  if [[ $LIST == 1 ]]; then
    higher="<cap+1>"
  elif [[ -n "$body" && "$cap" =~ ^([0-9]{1,15})(\.0+)?$ ]]; then
    higher=$((10#${BASH_REMATCH[1]} + 1))
  fi
  block_needs "$higher" "the runner limit shows no cap of a whole number of credits to raise"
  change_begin runner-cap limit "$body"
  [[ $LIST == 1 || -n "$BLOCK_SKIP" ]] || printf '%s\n' "$body" >"$H/runner-limit.json"
  step write net "Raise the runner spend cap" "one credit more; it is put back next" \
    -- hipc --json compute runners limit update --monthly-cap "$higher" --yes
  step read net "Read the runner limit" "confirm the server kept the new cap" \
    -- hipc --json compute runners limit get
  check online "the cap is the new one" -- shows "$RUNNER_CAP_IS" limit "$higher"
  step undo net "Put the runner spend cap back" "the server's own monthly_cap, read before" \
    -- hipc --json api PUT /runners/limit/ --body-file "$H/runner-limit.json" --yes
  local restored=$LAST_EXIT
  step read net "Read the runner limit" "confirm it is back" \
    -- hipc --json compute runners limit get
  check online "the cap is back" -- shows "$RUNNER_CAP_BACK" limit "$body"
  change_settle runner-cap limit "$restored" "$RUNNER_CAP_BACK" "$body"
  block_end
}

scenario_cdn() {
  local bucket="$1" zone_name="$2" waf_list="$3" cdn_bucket="$4" zone first_list
  begin_scenario cdn \
    "prices, an estimate, zones, hostnames and WAF; a zone is created only in the spend tier"
  login
  if [[ $LIST == 0 ]]; then
    printf '{"rules":[]}\n' >"$H/rules.json"
    # The API takes a request with exactly these ten keys, each one given.
    printf '%s\n' '{"ip":"203.0.113.7","country":"FR","asn":64496,"host":"hipc-manual.example",' \
      '"method":"GET","path":"/admin","query":"","headers":{"user-agent":"hipc-manual"},' \
      '"version":"HTTP/1.1","ssl":true}' | tr -d '\n' >"$H/waf-request.json"
    printf '203.0.113.7\n' >"$H/waf-items.txt"
    printf '198.51.100.0/24\n' >"$H/waf-items-2.txt"
    # A stand-in credential, never a real one; the dry run reads it and sends nothing.
    printf '{"api_token":"hipc-manual-test-not-a-cloudflare-token","zone_id":"z"}\n' \
      >"$H/dns-credentials.json"
    add_secret "hipc-manual-test-not-a-cloudflare-token" "<redacted>"
  fi
  step read net "Read the CDN prices" "see what traffic costs" -- hipc --output text cdn prices
  step read net "List the CDN regions" "see where the CDN serves from" \
    -- hipc --output text cdn regions
  # The one read that is not a GET: the estimate is a POST that prices a month and stores
  # nothing.
  step read net "Estimate a month" \
    "price 100 GB served in France and a million requests; a POST that changes nothing" \
    -- hipc --output text cdn estimate --traffic FR=100 --requests 1000000
  step read net "List the zones" "see the sites the account serves" -- hipc --json cdn zones list
  zone="$(pick "<zone>" "z000000000000" 'first(rows // [] | .[] | .id)')"
  step read net "List the WAF lists" "see the IP and country lists" -- hipc --json cdn lists list
  first_list="$(pick "<waf-list>" "blocked_ips" 'first(rows // [] | .[] | .name)')"
  [[ -n "$first_list" ]] || SKIP_REASON="the account has no WAF list"
  step read net "Read a WAF list" "see the first list's items" \
    -- hipc --output text cdn lists get "$first_list"
  step dry-run ok "Preview creating a zone" "see the request; a zone is billed for traffic" \
    -- hipc --output text cdn zones create --name "$zone_name" --bucket "$bucket" --dry-run
  if [[ $SPENDS == 1 ]]; then
    cdn_spend "$zone_name" "$cdn_bucket"
  else
    block_needs "$zone" "the account has no CDN zone"
    zone_reads "$zone"
    block_end
  fi
  zone_dry_runs "${zone:-z000000000000}"
  if [[ $WRITES == 0 ]]; then
    waf_list_dry_runs "$waf_list"
  else
    waf_list_writes "$waf_list"
  fi
  logout
  end_scenario
}

# The reads of one zone. In the spend tier the zone is the one it created.
zone_reads() {
  local zone="$1" version purge
  step read net "Read the zone" "see its bucket, hostnames and settings" \
    -- hipc --output text cdn zones get "$zone"
  step read net "Read the zone's spend cap" "see the most its traffic may cost a month" \
    -- hipc --output text cdn cap get "$zone"
  step read net "List the zone's hostnames" "see the custom hostnames and their DNS state" \
    -- hipc --output text cdn hostnames list "$zone"
  step read net "Read the zone's rules" "see its redirects and headers" \
    -- hipc --output text cdn rules get "$zone"
  step read net "Read the zone's traffic" "see requests and bytes over time" \
    -- hipc --output text cdn stats "$zone"
  step read net "Read a zone's WAF settings" "see the firewall mode of the zone" \
    -- hipc --output text cdn waf get "$zone"
  step read net "Read the WAF status" "see whether the published rules are live" \
    -- hipc --output text cdn waf status "$zone"
  step read net "Read the WAF analytics" "see what the firewall did, by rule" \
    -- hipc --output text cdn waf analytics "$zone"
  step read net "List the WAF events" "see the requests the firewall acted on" \
    -- hipc --output text cdn waf events "$zone"
  step read net "Read the WAF rules" "see the custom rules" \
    -- hipc --output text cdn waf rules get "$zone"
  step read net "Read the rate limits" "see the request limits" \
    -- hipc --output text cdn waf rate-limits get "$zone"
  # Testing a request needs CDN operator access, which an ordinary account lacks.
  step read net/permission,insufficient_grant "Test a request against the WAF" \
    "ask which published rules a request would match; changes nothing" \
    -- hipc --json cdn waf test "$zone" --request-file "$H/waf-request.json"
  step read net "List the WAF versions" "see the published rule versions" \
    -- hipc --json cdn waf versions list "$zone"
  version="$(pick "<version>" "1" \
    'first(rows // [] | .[] | .version_number // .version // .number)')"
  [[ -n "$version" ]] || SKIP_REASON="the zone has no WAF version"
  step read net "Read a WAF version" "see what the first version published" \
    -- hipc --output text cdn waf versions get "$zone" "${version:-1}"
  step read net "List the purges" "see the cache purges and their state" \
    -- hipc --json cdn purges list "$zone"
  purge="$(pick "<purge>" "$MISSING_NUMBER" 'first(rows // [] | .[] | .id)')"
  [[ -n "$purge" ]] || SKIP_REASON="the zone has no purge"
  step read net "Read a purge" "see the first purge's state" \
    -- hipc --output text cdn purges get "$zone" "${purge:-$MISSING_NUMBER}"
  # Domain Connect answers 404 when the hostname's DNS provider does not offer it, as for
  # an example hostname.
  step read net/not_found "Read a Domain Connect link" \
    "see the link a provider that offers Domain Connect would answer" \
    -- hipc --json cdn hostnames domain-connect "$zone" hipc-manual.example.com
}

# The zone writes that would change a live site, as dry runs in every tier.
zone_dry_runs() {
  local zone="$1"
  step dry-run ok "Preview a zone change" "see the request; nothing changes" \
    -- hipc --output text cdn zones update "$zone" --redirect-https true --yes --dry-run
  step dry-run ok "Preview a spend cap" "see the request" \
    -- hipc --output text cdn cap update "$zone" --monthly-cap 1 --yes --dry-run
  step dry-run ok "Preview adding a hostname" "see the request; it needs a domain the owner has" \
    -- hipc --output text cdn hostnames create "$zone" hipc-manual.example.com --yes --dry-run
  step dry-run ok "Preview verifying a hostname" "see the request" \
    -- hipc --output text cdn hostnames verify "$zone" hipc-manual.example.com --dry-run
  step dry-run ok "Preview writing the DNS records" \
    "see the request; the credential file is a stand-in and is never printed" \
    -- hipc --output text cdn hostnames dns-setup "$zone" hipc-manual.example.com \
    --provider cloudflare --credentials-file "$H/dns-credentials.json" --yes --dry-run
  step dry-run ok "Preview removing a hostname" "see the request" \
    -- hipc --output text cdn hostnames delete "$zone" hipc-manual.example.com --yes --dry-run
  step dry-run ok "Preview a purge" "see the request; a purge empties the cache" \
    -- hipc --output text cdn purges create "$zone" --everything --dry-run
  step dry-run ok "Preview replacing the rules" "see the request; nothing is published" \
    -- hipc --output text cdn rules update "$zone" --rules-file "$H/rules.json" \
    --remove-others --yes --dry-run
  step dry-run ok "Preview a WAF mode" "see the request" \
    -- hipc --output text cdn waf update "$zone" --mode log --yes --dry-run
  step dry-run ok "Preview replacing the WAF rules" "see the request; nothing is published" \
    -- hipc --output text cdn waf rules update "$zone" --rules-file "$H/rules.json" \
    --remove-others --yes --dry-run
  step dry-run ok "Preview replacing the rate limits" "see the request; nothing is published" \
    -- hipc --output text cdn waf rate-limits update "$zone" --rules-file "$H/rules.json" \
    --remove-others --yes --dry-run
  step dry-run ok "Preview a WAF rollback" "see the request; nothing is published" \
    -- hipc --output text cdn waf versions rollback "$zone" 1 --yes --dry-run
  step dry-run ok "Preview deleting the zone" "see the request; nothing is deleted" \
    -- hipc --output text cdn zones delete "$zone" --yes --dry-run
}

waf_list_dry_runs() {
  local name="$1"
  step dry-run ok "Preview a WAF list" "see the request; the read tier creates nothing" \
    -- hipc --output text cdn lists create "$name" --kind ip --items-file "$H/waf-items.txt" \
    --dry-run
  step dry-run ok "Preview a list description" "see the request" \
    -- hipc --output text cdn lists update "$name" --description "hipc manual test" --dry-run
  step dry-run ok "Preview adding items" "see the request" \
    -- hipc --output text cdn lists items create "$name" --items-file "$H/waf-items-2.txt" \
    --yes --dry-run
  step dry-run ok "Preview replacing the items" "see the request" \
    -- hipc --output text cdn lists items update "$name" --items-file "$H/waf-items.txt" \
    --remove-others --yes --dry-run
  step dry-run ok "Preview removing items" "see the request" \
    -- hipc --output text cdn lists items delete "$name" --items-file "$H/waf-items.txt" \
    --yes --dry-run
  step dry-run ok "Preview deleting the list" "see the request" \
    -- hipc --output text cdn lists delete "$name" --yes --dry-run
}

# A WAF list is free and used by no zone until a rule names it: the write tier makes one,
# edits it, and deletes it. Its id is its name.
waf_list_writes() {
  local name="$1" created
  ledger_add cdn-list "$name"
  step write net "Create a WAF list" "an IP list for this test, with one documentation address" \
    -- hipc --output text cdn lists create "$name" --kind ip --items-file "$H/waf-items.txt"
  created=$LAST_EXIT
  ledger_created cdn-list "$name" "$created" "$name"
  step write net "Describe the list" "set its note" \
    -- hipc --output text cdn lists update "$name" --description "hipc manual test"
  step write net "Add items" "add a documentation network" \
    -- hipc --output text cdn lists items create "$name" --items-file "$H/waf-items-2.txt" --yes
  step write net "Replace the items" "keep only the first address" \
    -- hipc --output text cdn lists items update "$name" --items-file "$H/waf-items.txt" \
    --remove-others --yes
  step write net "Remove items" "empty the list" \
    -- hipc --output text cdn lists items delete "$name" --items-file "$H/waf-items.txt" --yes
  step read net "Read the list" "confirm it is empty" -- hipc --json cdn lists get "$name"
  undo_needs "$created" "$name"
  step undo net "Delete the list" "undo the create" \
    -- hipc --output text cdn lists delete "$name" --yes
  local deleted=$LAST_EXIT
  step read net "List the WAF lists" "confirm the list is gone" -- hipc --json cdn lists list
  check online "the list no longer shows the WAF list" -- not_listed "$name"
  ledger_settle cdn-list "$name" "$deleted"
}

# The spend tier's zone: a new bucket served by a new zone with a 1-credit monthly cap, so
# its traffic cannot cost more; the zone is read and changed, purged, then deleted, then the
# bucket. Nothing is uploaded, so there is no traffic to bill.
cdn_spend() {
  local zone_name="$1" cdn_bucket="$2" bucket_created created
  SPEND_OK=0
  spend_gate "a CDN zone on a new, empty bucket, with a 1-credit cap, deleted at the end" \
    "at most 1 credit: the zone is billed for traffic, and nothing is uploaded or served"
  ledger_add bucket "$cdn_bucket"
  step write net "Create a bucket for the zone" "a new, empty bucket for this test" \
    -- hipc --output text s3 buckets create "$cdn_bucket" --yes
  bucket_created=$LAST_EXIT
  ledger_created bucket "$cdn_bucket" "$bucket_created" "$cdn_bucket"
  ledger_add cdn-zone "$zone_name"
  step spend net "Create a zone" "serve the empty bucket; --public says the objects become public" \
    -- hipc --json cdn zones create --name "$zone_name" --bucket "$cdn_bucket" --public --yes
  created=$LAST_EXIT
  zone="$(pick "<new-zone>" "z000000000000" '.id')"
  ledger_created cdn-zone "$zone_name" "$created" "$zone"
  [[ -n "$BLOCK_SKIP" ]] || bought "$created"
  if [[ $LIST == 0 && $OFFLINE == 0 && -z "$BLOCK_SKIP" && ($created != 0 || -z "$zone") ]]; then
    BLOCK_SKIP="the create did not answer a zone id; the cleanup ledger looks for it by name"
  fi
  step write net "Cap the zone's spend" "at most 1 credit a month" \
    -- hipc --output text cdn cap update "$zone" --monthly-cap 1 --yes
  step write net "Redirect to HTTPS" "change one setting of the new zone" \
    -- hipc --output text cdn zones update "$zone" --redirect-https true --yes
  step write net "Put the WAF in log mode" "the firewall logs and blocks nothing" \
    -- hipc --output text cdn waf update "$zone" --mode log --yes
  step write net "Purge the cache" "empty the new zone's cache; there is nothing in it" \
    -- hipc --output text cdn purges create "$zone" --everything
  zone_reads "$zone"
  step undo net "Delete the zone" "undo the create: billing stops" \
    -- hipc --output text cdn zones delete "$zone" --yes
  local deleted=$LAST_EXIT
  step read net "List the zones" "confirm the zone is gone" -- hipc --json cdn zones list
  check online "the list no longer shows the zone" -- not_listed "$zone_name"
  [[ -n "$BLOCK_SKIP" ]] || ledger_settle cdn-zone "$zone_name" "$deleted"
  block_end
  undo_needs "$bucket_created" "$cdn_bucket"
  [[ $LIST == 1 || $SPEND_OK == 1 ]] || SKIP_REASON="the purchase was declined"
  step undo net "Delete the zone's bucket" "undo the create" \
    -- hipc --output text s3 buckets delete "$cdn_bucket" --yes
  local bucket_deleted=$LAST_EXIT
  [[ $LIST == 1 || $SPEND_OK == 1 ]] || SKIP_REASON="the purchase was declined"
  step read net "List the buckets" "confirm the bucket is gone" -- hipc --json s3 buckets list
  [[ $SPEND_OK == 0 ]] || ledger_settle bucket "$cdn_bucket" "$bucket_deleted"
}

# The private network and managed Kubernetes, reads only. `network get` is a dry run in
# every tier: the first one for an account creates its network. The kubeconfig dry run
# needs a key file, which hipc reads before it decides to send nothing, so the scenario
# writes one with the console's public test vector (seeds 11..42 and 1..32, never a real
# key) in the temporary home, under the script's umask 077.
scenario_network_kubernetes() {
  local cluster key peer vpc
  begin_scenario network-kubernetes \
    "the private network and Kubernetes; the network summary and a kubeconfig are dry runs"
  # After begin_scenario, which sets H to this scenario's home.
  key="$H/hippius-cluster-manual.key"
  login
  step dry-run ok "Preview the network summary" \
    "see the request; the first get for an account creates its network, so it is never sent here" \
    -- hipc --output text network get --dry-run
  step dry-run ok "Preview creating the network" \
    "see the request; a network cannot be removed once it is set up, so no tier sends it" \
    -- hipc --output text network create --yes --dry-run
  # An account with no network yet, or no Kubernetes access, may answer not_found; one whose
  # network was never set up answers network_unprovisioned (see error_code).
  step read net/not_found,network_unprovisioned "List the peers" \
    "see the devices and VMs enrolled in the network" \
    -- hipc --json network peers list
  peer="$(pick "<peer>" "$MISSING_NUMBER" 'first(rows // [] | .[] | .id)')"
  [[ -n "$peer" ]] || SKIP_REASON="the network has no peer"
  step read net "Read a peer" "see the first peer's addresses and groups" \
    -- hipc --json network peers get "${peer:-$MISSING_NUMBER}"
  # A peer's name is its DNS name in the network, which a person or a service may be using,
  # so the rename is a dry run in every tier.
  step dry-run ok "Preview renaming a peer" "see the request; nothing is renamed" \
    -- hipc --output text network peers update "${peer:-$MISSING_NUMBER}" --name hipc-manual \
    --yes --dry-run
  step dry-run ok "Preview removing a peer" "see the request; the device would leave the network" \
    -- hipc --output text network peers delete "${peer:-$MISSING_NUMBER}" --yes --dry-run
  step read net/not_found,network_unprovisioned "List the peer groups" \
    "see the groups the policies name" \
    -- hipc --json network groups list
  step read net/not_found,network_unprovisioned "List the network policies" \
    "see which groups may reach which" \
    -- hipc --json network policies list
  step read net/not_found,network_unprovisioned "List the VPC networks" \
    "see the private networks and their CIDRs" \
    -- hipc --json network vpcs list
  vpc="$(pick "<vpc>" "$MISSING_NUMBER" 'first(rows // [] | .[] | .id)')"
  block_needs "$vpc" "the account has no VPC network"
  step read net "List a VPC's resources" "see the VMs and services attached to the first one" \
    -- hipc --output text network vpcs resources list "$vpc"
  step read net "List a VPC's routers" "see its routers and their addresses" \
    -- hipc --output text network vpcs routers list "$vpc"
  block_end
  step read net/not_found "List the Kubernetes clusters" \
    "see the managed clusters and their state" \
    -- hipc --json compute kubernetes clusters list
  cluster="$(pick "<cluster>" "$MISSING_NUMBER" 'first(rows // [] | .[] | .id)')"
  block_needs "$cluster" "the account has no Kubernetes cluster"
  cluster_reads "$cluster"
  block_end
  # A dry run sends nothing, so an id that names no cluster serves when the account has none.
  [[ -n "$cluster" ]] || cluster="$MISSING_NUMBER"
  step read net/not_found "List the refused launches" \
    "see the cluster launches refused in the last 7 days" \
    -- hipc --json compute kubernetes clusters refusals
  [[ $LIST == 1 ]] || write_test_cluster_key "$key"
  check always "the key file is in this scenario's home" -- key_in_home "$key"
  cluster_dry_runs "$cluster" "$key"
  step dry-run ok "Preview fetching a kubeconfig" \
    "see the two reads a kubeconfig needs; the file itself would go only to --output-file" \
    -- hipc --output text compute kubernetes clusters kubeconfig "$cluster" \
    --cluster-key-file "$key" --output-file "$H/kubeconfig" --dry-run
  check always "the dry run wrote no kubeconfig" -- test ! -e "$H/kubeconfig"
  logout
  end_scenario
}

cluster_reads() {
  local cluster="$1"
  step read net "Read the cluster" "see the first cluster's version and state" \
    -- hipc --output text compute kubernetes clusters get "$cluster"
  step read net "List the cluster's events" "see what happened to it lately" \
    -- hipc --output text compute kubernetes clusters events "$cluster"
  # The answer's download links fetch a snapshot without a token, so it needs --reveal; the
  # mask hides every download link.
  step read net "List the cluster's backups" "see its etcd snapshots; the links are masked" \
    -- hipc --json compute kubernetes backups list "$cluster" --reveal
  step read net "List the node pools" "see the pools and their sizes" \
    -- hipc --json compute kubernetes node-pools list "$cluster"
  step read net "List the operations" "see the changes running or blocked on the cluster" \
    -- hipc --output text compute kubernetes operations list "$cluster"
}

# Every cluster write as a dry run: none is undone within the run, and there is no
# cluster create to make a throwaway one. The node writes need the key file, which the dry
# run reads and never prints.
cluster_dry_runs() {
  local cluster="$1" key="$2"
  step dry-run ok "Preview a node pool" "see the request; a pool starts empty" \
    -- hipc --output text compute kubernetes node-pools create "$cluster" --name hipc-manual \
    --flavor small --dry-run
  step dry-run ok "Preview scaling a pool" "see the request; nodes are billed" \
    -- hipc --output text compute kubernetes node-pools scale "$cluster" "$MISSING_NUMBER" \
    --count 1 --cluster-key-file "$key" --yes --dry-run
  step dry-run ok "Preview cancelling an operation" "see the request" \
    -- hipc --output text compute kubernetes operations cancel "$cluster" "$MISSING_NUMBER" \
    --yes --dry-run
  step dry-run ok "Preview resuming an operation" "see the request" \
    -- hipc --output text compute kubernetes operations resume "$cluster" "$MISSING_NUMBER" \
    --yes --dry-run
  step dry-run ok "Preview replacing a node" "see the request; the node is drained first" \
    -- hipc --output text compute kubernetes nodes replace "$cluster" hipc-manual-node \
    --cluster-key-file "$key" --yes --dry-run
  step dry-run ok "Preview removing a node" "see the request" \
    -- hipc --output text compute kubernetes nodes delete "$cluster" hipc-manual-node --yes \
    --dry-run
  step dry-run ok "Preview deleting the cluster" "see the request; nothing is deleted" \
    -- hipc --output text compute kubernetes clusters delete "$cluster" --yes --dry-run
}

# The console's public test vector for a cluster key file, as scripts/release-check.sh
# uses it: the seeds 11..42 and 1..32, never a real key. Both seeds are in the mask, and
# the teardown scans every step's raw output for them (see scan_key_outputs).
TEST_KEY_SIGN_SEED="CwwNDg8QERITFBUWFxgZGhscHR4fICEiIyQlJicoKSo="
TEST_KEY_BOX_SEED="AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA="
write_test_cluster_key() {
  # The first write creates the file under umask 077, so it is 0600 as hipc requires.
  printf '{"version":1,"kind":"hippius-k8s-cluster-key",' >"$1"
  printf '"sign_secret_key":"%s","sign_public_key":"%s",' \
    "$TEST_KEY_SIGN_SEED" "VCgh6VKd0a0kO/3/XGQKZRlLSBaYemUN6GHXzp5BemM=" >>"$1"
  printf '"box_secret_key":"%s","box_public_key":"%s"}\n' \
    "$TEST_KEY_BOX_SEED" "B6N8vBQgk8i3VdwbEOhstCY3StFqqFPtC9/AsrhtHHw=" >>"$1"
}
# The key path is built after begin_scenario sets H; a path under an earlier home would
# still pass the catalogue pin, where every home is `~`, so it is checked at run time.
key_in_home() { [[ "$1" == "$H/"* && -f "$1" ]]; }

scenario_team() {
  local project_name="$1" project named billing billing_id rename_id rename_name member invite
  begin_scenario team "members, invites, projects and billing accounts; pick one, then reset"
  login
  step read net "List the members" "see who is in the team" -- hipc --json account members list
  member="$(pick "<member>" "$MISSING_NUMBER" 'first(rows // [] | .[] | .id)')"
  [[ -n "$member" ]] || member="$MISSING_NUMBER"
  # Member changes reach a real person, so they are dry runs in every tier.
  step dry-run ok "Preview a member change" "see the request; nothing changes" \
    -- hipc --output text account members update "$member" --manage-team false --yes --dry-run
  step dry-run ok "Preview removing a member" "see the request; nobody is removed" \
    -- hipc --output text account members delete "$member" --yes --dry-run
  step read net "List the invites" "see invites not yet accepted" \
    -- hipc --json account invites list
  invite="$(pick "<invite>" "$MISSING_NUMBER" 'first(rows // [] | .[] | .id)')"
  [[ -n "$invite" ]] || invite="$MISSING_NUMBER"
  step dry-run ok "Preview an invite" "see the request; an invite emails a real address" \
    -- hipc --output text account invites create --email hipc-manual@example.com --dry-run
  step dry-run ok "Preview revoking an invite" "see the request; nothing is revoked" \
    -- hipc --output text account invites revoke "$invite" --yes --dry-run
  # An invite token is a secret emailed to the invited person; this one is a stand-in.
  if [[ $LIST == 0 ]]; then
    printf 'hipc-manual-test-not-an-invite-token\n' >"$H/invite.token"
    add_secret "hipc-manual-test-not-an-invite-token" "<redacted>"
  fi
  step dry-run ok "Preview reading an invite" "see the request an emailed invite token makes" \
    -- hipc --output text account invites preview --token-file "$H/invite.token" --yes --dry-run
  step dry-run ok "Preview accepting an invite" "see the request; nothing is joined" \
    -- hipc --output text account invites accept --token-file "$H/invite.token" --yes --dry-run
  step read net "List the projects" "see the projects in the account" \
    -- hipc --json account projects list
  project="$(pick "<project>" "default" 'first(rows // [] | .[] | .id)')"
  [[ -n "$project" ]] || project="default"
  # The Default project has no id the budget and quota routes take, so they read the first
  # project that has one, and are skipped when there is none.
  named="$(pick "<project>" "$MISSING_PROJECT" 'first(rows // [] | .[] | .id | select(. != null))')"
  step dry-run ok "Preview a project" "see the request; a project is free, so it is not confirmed" \
    -- hipc --output text account projects create --name hipc-manual --dry-run
  [[ -n "$named" ]] || SKIP_REASON="the account has no project but the Default"
  step read net "Read a project's budget" "see its monthly limit and this month's spend" \
    -- hipc --output text account projects budget get "${named:-$MISSING_PROJECT}"
  [[ -n "$named" ]] || SKIP_REASON="the account has no project but the Default"
  step read net "Read a project's quota" "see its own limits beside the account's" \
    -- hipc --output text account projects quota get "${named:-$MISSING_PROJECT}"
  if [[ $WRITES == 0 ]]; then
    project_dry_runs "${named:-$MISSING_PROJECT}"
  else
    project_writes "$project_name"
  fi
  step dry-run ok "Preview a project transfer" \
    "see the request; a transfer gives the project to another account" \
    -- hipc --output text account projects transfer "${named:-$MISSING_PROJECT}" \
    --to-account "$MISSING_ACCOUNT" --yes --dry-run
  step read net "List the project transfers" "see projects offered to or by the own account" \
    -- hipc --output text account project-transfers list
  # A transfer offered to this account is the other account's to make, so the answers to one
  # are dry runs, by an id that names none.
  step dry-run ok "Preview accepting a transfer" "see the request" \
    -- hipc --output text account project-transfers accept "$MISSING_NUMBER" --yes --dry-run
  step dry-run ok "Preview declining a transfer" "see the request" \
    -- hipc --output text account project-transfers decline "$MISSING_NUMBER" --yes --dry-run
  step dry-run ok "Preview cancelling a transfer" "see the request" \
    -- hipc --output text account project-transfers cancel "$MISSING_NUMBER" --yes --dry-run
  step read net "List the billing accounts" "see which billing accounts pay" \
    -- hipc --json account billing-accounts list
  billing="$(pick "<billing-account>" "default" 'first(rows // [] | .[] | .id)')"
  [[ -n "$billing" ]] || billing="default"
  # The billing account a write tier rename takes: the first with an id, and its name.
  rename_id="$(pick "<billing-account>" "$MISSING_BILLING" \
    'first(rows // [] | .[] | objects | select((.id | tostring) | startswith("bill_"))) | .id')"
  rename_name="$(pick "<billing-name>" "hipc-manual-offline" \
    'first(rows // [] | .[] | objects | select((.id | tostring) | startswith("bill_")))
      | .name | strings')"
  # The Default billing account has no id the costs and update routes take.
  billing_id="$MISSING_BILLING"
  [[ "$billing" != bill_* && $LIST == 0 ]] || billing_id="$billing"
  [[ "$billing" != default ]] || SKIP_REASON="the account has only the Default billing account"
  step read net "Read a billing account's costs" "see what its projects cost, by project" \
    -- hipc --output text account billing-accounts costs "$billing" --group-by project
  # There is no delete for a billing account, so its create is a dry run in every tier. The
  # write tier renames the first one and renames it back.
  step dry-run ok "Preview a billing account" "see the request; a billing account stays" \
    -- hipc --output text account billing-accounts create --name hipc-manual --dry-run
  if [[ $WRITES == 0 ]]; then
    step dry-run ok "Preview renaming a billing account" "see the request; nothing is renamed" \
      -- hipc --output text account billing-accounts update "$billing_id" --name hipc-manual \
      --dry-run
  else
    billing_rename "$rename_id" "$rename_name"
  fi
  step local ok "Pick a project" "act in the first project from now on" \
    -- hipc --output text account projects use "$project"
  step local ok "Pick the Default project again" "reset the choice" \
    -- hipc --output text account projects use default
  step local ok "Pick a billing account" "pay from the first billing account from now on" \
    -- hipc --output text account billing-accounts use "$billing"
  step local ok "Pick the Default billing account again" "reset the choice" \
    -- hipc --output text account billing-accounts use default
  step local ok "Act in the own account" "clear any account choice" \
    -- hipc --output text account use self
  logout
  end_scenario
}

# The write tier renames the first billing account that has an id (not the Default one) and
# renames it back, reading the list each time; there is no read of one billing account. A
# name hipc would not send back as it is (one it trims, or one its --name refuses, such as an
# invisible character or over 64 characters) is left alone: the put-back would fail.
billing_rename() {
  local billing="$1" name="$2"
  block_needs "$billing" "the account has only the Default billing account"
  [[ -n "$BLOCK_SKIP" ]] || block_needs "$name" "the billing account has no name to put back"
  if [[ $LIST == 0 && -z "$BLOCK_SKIP" ]] && { [[ "$name" =~ ^[[:space:]]|[[:space:]]$ ]] ||
    ! "$HIPC" --json --dry-run account billing-accounts update "$billing" --name "$name" \
      </dev/null >/dev/null 2>&1; }; then
    BLOCK_SKIP="hipc would not send the billing account's name back as it is"
  fi
  change_begin billing-name "$billing" "$name"
  step write net "Rename a billing account" "a name unique to this run; the old one is put back next" \
    -- hipc --json account billing-accounts update "$billing" --name "$RENAME"
  step read net "List the billing accounts" "confirm the server kept the new name" \
    -- hipc --json account billing-accounts list
  check online "the billing account has the new name" \
    -- shows "$BILLING_NAME_IS" "$billing" "$RENAME"
  step undo net "Rename the billing account back" "the name read before the change" \
    -- hipc --json account billing-accounts update "$billing" --name "$name"
  local restored=$LAST_EXIT
  step read net "List the billing accounts" "confirm the old name is back" \
    -- hipc --json account billing-accounts list
  check online "the billing account has its old name" \
    -- shows "$BILLING_NAME_IS" "$billing" "$name"
  change_settle billing-name "$billing" "$restored" "$BILLING_NAME_IS" "$name"
  block_end
}

project_dry_runs() {
  local project="$1"
  step dry-run ok "Preview renaming a project" "see the request; nothing is renamed" \
    -- hipc --output text account projects update "$project" --name hipc-manual --dry-run
  step dry-run ok "Preview a project budget" "see the request" \
    -- hipc --output text account projects budget update "$project" --monthly-limit 1 \
    --alert-at 80 --block-launches false --yes --dry-run
  step dry-run ok "Preview removing the budget" "see the request" \
    -- hipc --output text account projects budget delete "$project" --yes --dry-run
  step dry-run ok "Preview a project quota" "see the request" \
    -- hipc --output text account projects quota update "$project" --vms 1 --yes --dry-run
  step dry-run ok "Preview archiving a project" "see the request; nothing is archived" \
    -- hipc --output text account projects delete "$project" --yes --dry-run
}

# A project is free: the write tier makes one, renames it, gives it a budget and a quota,
# takes the budget off, and archives it. An archived project is gone for the snapshot.
project_writes() {
  local name="$1" created id
  ledger_add project "$name"
  step write net "Create a project" "a project for this test, with a unique name" \
    -- hipc --json account projects create --name "$name"
  created=$LAST_EXIT
  id="$(pick "<new-project>" "$MISSING_PROJECT" '.id')"
  ledger_created project "$name" "$created" "$id"
  undo_needs "$created" "$id"
  step write net "Rename the project" "a new name, which the ledger then follows" \
    -- hipc --output text account projects update "${id:-$MISSING_PROJECT}" --name "$name-renamed"
  if [[ $LIST == 0 && $LAST_EXIT == 0 ]]; then
    ledger_rename project "$name" "$name-renamed" "$id"
    name="$name-renamed"
  fi
  undo_needs "$created" "$id"
  step write net "Give the project a budget" "1 USD a month, an alert at 80%, no block" \
    -- hipc --output text account projects budget update "${id:-$MISSING_PROJECT}" \
    --monthly-limit 1 --alert-at 80 --block-launches false --yes
  undo_needs "$created" "$id"
  step read net "Read the budget" "confirm the limit" \
    -- hipc --output text account projects budget get "${id:-$MISSING_PROJECT}"
  undo_needs "$created" "$id"
  step undo net "Remove the budget" "undo it" \
    -- hipc --output text account projects budget delete "${id:-$MISSING_PROJECT}" --yes
  undo_needs "$created" "$id"
  step write net "Set a project quota" "at most one VM in the project" \
    -- hipc --output text account projects quota update "${id:-$MISSING_PROJECT}" --vms 1 --yes
  undo_needs "$created" "$id"
  step undo net "Archive the project" "undo the create; it owns nothing" \
    -- hipc --output text account projects delete "${id:-$MISSING_PROJECT}" --yes
  local deleted=$LAST_EXIT
  step read net "List the projects" "confirm the project is gone or archived" \
    -- hipc --json account projects list
  check online "the list no longer shows the project as live" -- not_listed "$name"
  ledger_settle project "$name" "$deleted"
}

scenario_support() {
  local tag="$1" ticket
  begin_scenario support \
    "read the tickets and their messages; opening or closing one is a dry run below spend"
  login
  step read net "List the tickets" "see open and closed support tickets" \
    -- hipc --json support tickets list --limit 5
  ticket="$(pick "<ticket>" "$MISSING_NUMBER" 'first(rows // [] | .[] | .id)')"
  [[ -n "$ticket" ]] || SKIP_REASON="the account has no ticket"
  step read net "Read a ticket" "see the first ticket and its replies" \
    -- hipc --output text support tickets get "$ticket"
  [[ -n "$ticket" ]] || SKIP_REASON="the account has no ticket"
  step read net "Read a ticket's messages" "see the thread of the first ticket, page by page" \
    -- hipc --output text support tickets messages list "$ticket" --limit 5
  [[ $LIST == 1 ]] || printf 'A test message from scripts/manual_test.sh. Do not send.\n' \
    >"$H/ticket.txt"
  step dry-run ok "Preview opening a ticket" \
    "see the request; a person on the support team would read a real one" \
    -- hipc --output text support tickets create --subject "hipc manual test" \
    --body-file "$H/ticket.txt" --dry-run
  step dry-run ok "Preview closing a ticket" \
    "see the read of the ticket, then the close; a close ends the support team's work on it" \
    -- hipc --output text support tickets update "${ticket:-$MISSING_NUMBER}" --status closed \
    --dry-run
  step dry-run ok "Preview a reply" "see the request; a reply reaches the support team" \
    -- hipc --output text support tickets reply "${ticket:-$MISSING_NUMBER}" \
    --body-file "$H/ticket.txt" --dry-run
  [[ $SPENDS == 0 ]] || ticket_spend "$tag"
  logout
  end_scenario
}

# The spend tier's ticket: a real one, which a person on the support team reads, so its
# subject says what it is and asks them to close it. The run replies once, then closes it
# itself when this hipc has `support tickets update`. The ticket stays in the list, closed:
# the snapshot allows exactly the tickets this run opened.
ticket_spend() {
  local tag="$1" opened id subject
  SPEND_OK=0
  subject="CLI release test $tag: no help needed, please close this ticket"
  spend_gate "a real support ticket, closed at the end" \
    "no credits; a person on the support team reads it"
  if [[ $LIST == 0 ]]; then
    printf '%s\n\n%s\n' \
      "The hipc command-line tool's pre-release test (scripts/manual_test.sh) opened this ticket." \
      "Nobody needs help. Please close it; the test also closes it itself where it can." \
      >"$H/ticket-real.txt"
    printf 'The pre-release test checks that a reply is delivered. No answer is needed.\n' \
      >"$H/ticket-reply.txt"
  fi
  ledger_add ticket "$tag"
  step spend net "Open a ticket" "a real ticket whose subject asks the team to close it" \
    -- hipc --json support tickets create --subject "$subject" --body-file "$H/ticket-real.txt" \
    --yes
  opened=$LAST_EXIT
  id="$(pick "<new-ticket>" "$MISSING_NUMBER" '.id')"
  ledger_created ticket "$tag" "$opened" "$id"
  # Only a ticket this run opened, and whose id it knows, may appear in the snapshot diff.
  if [[ $LIST == 0 && -z "$BLOCK_SKIP" && $opened == 0 && -n "$(answered '.id')" ]]; then
    printf 'ticket %s\n' "$(answered '.id')" >>"$WORK/spent"
  elif [[ $LIST == 0 && $OFFLINE == 0 && -z "$BLOCK_SKIP" ]]; then
    BLOCK_SKIP="the create did not answer a ticket id; its subject asks the team to close it"
  fi
  step write net "Reply to the ticket" "a reply reaches the support team too" \
    -- hipc --output text support tickets reply "$id" --body-file "$H/ticket-reply.txt"
  # The close needs `support tickets update`; a binary without it lists no close, and at run
  # time the step is skipped, with the subject asking the team to close the ticket.
  if [[ $LIST == 0 ]] || has_command "support tickets update"; then
    has_command "support tickets update" ||
      SKIP_REASON="this hipc has no \`support tickets update\`; the subject asks for a close"
    step undo net "Close the ticket" "undo the create, as a customer closes a ticket" \
      -- hipc --output text support tickets update "$id" --status closed --yes
    local closed=$LAST_EXIT
    has_command "support tickets update" ||
      SKIP_REASON="this hipc has no \`support tickets update\`; the ticket stays open"
    step read net "Read the ticket" "confirm it is closed" -- hipc --json support tickets get "$id"
    if [[ $LIST == 0 && $OFFLINE == 0 && $closed == 0 ]] && undone ticket "$LAST_OUT"; then
      ledger_remove ticket "$tag"
    fi
  fi
  block_end
}

scenario_agent() {
  local bucket="$1"
  begin_scenario agent "JSON output, the schema, --all paging and the error codes"
  login
  step local ok "Read the schema" "an agent learns every command, flag and error code" \
    -- hipc schema
  check always "the schema lists the commands and the error codes" -- schema_lists_commands
  step read net "List VMs as an agent" "AI_AGENT makes the output JSON with no flag" \
    -- AI_AGENT=1 hipc compute vms list
  check always "the answer is JSON" -- answer_is_json
  step read net "Read every page of a list" "--all follows every page into one array" \
    -- hipc --json compute ssh-keys list --all
  check online "the answer is one array" -- stdout_is_array
  step read net "Call a route directly" "hipc api sends a GET with the token and prints it" \
    -- hipc --json api GET /api/services/status/
  check always "the answer is JSON" -- answer_is_json
  step error err:confirmation_required "Send a write through hipc api without --yes" \
    "a method other than GET or HEAD is confirmed, so nothing is sent" \
    -- hipc --json api DELETE "/api/objectstore/buckets/$bucket/"
  step error net-err:auth "Use a bad token" "a wrong token is exit 1 with the code auth" \
    -- "HIPPIUS_TOKEN=$BAD_TOKEN" hipc --json auth status
  step error err:confirmation_required "Delete without --yes" \
    "a delete without --yes is refused locally, exit 2, and nothing is sent" \
    -- hipc --json s3 buckets delete "$bucket"
  step error err:reveal_required "Create a token without --reveal" \
    "a command that returns a secret needs --reveal, exit 2" \
    -- hipc --json s3 tokens create --kind sub --bucket "$bucket" --yes
  step error err:usage "Pass a flag that does not exist" "a bad flag is exit 2 with the code usage" \
    -- hipc --json compute vms list --bogus
  step dry-run ok "Preview a write" "a dry run shows the request as JSON and exits 0" \
    -- hipc --json s3 buckets create "$bucket" --dry-run
  logout
  end_scenario
}

scenario_token_hygiene() {
  begin_scenario token-hygiene "where the token goes, and where it never goes"
  login
  check always "only the owner can read the config file" -- config_private
  step read net "Check the login as JSON" "the status shows a preview, never the token" \
    -- hipc --json auth status
  check always "stdout and stderr do not hold the token" -- no_token_in_last
  step read net "Read with --debug" "the request lines name no header and no token" \
    -- hipc --debug --output text account list
  check always "stdout and stderr do not hold the token" -- no_token_in_last
  logout
  step error net-err:auth "Use a bad token from the environment" \
    "a bad HIPPIUS_TOKEN is exit 1 with the code auth, and it is not stored" \
    -- "HIPPIUS_TOKEN=$BAD_TOKEN" hipc --json account list
  check always "the config file does not hold the bad token" -- no_bad_token_stored
  end_scenario
}

no_token_in_last() { no_token_in "$LAST_OUT" && no_token_in "$LAST_ERR"; }
no_bad_token_stored() {
  [[ ! -e "$H/.hippius/config.toml" ]] || ! grep -qF -- "$BAD_TOKEN" "$H/.hippius/config.toml"
}

# ---------------------------------------------------------------------------------------
# Setup: the binary, the error table, and the snapshot login.

release_target() {
  case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) echo aarch64-apple-darwin ;;
  Darwin/x86_64) echo x86_64-apple-darwin ;;
  Linux/x86_64) echo x86_64-unknown-linux-musl ;;
  Linux/aarch64 | Linux/arm64) echo aarch64-unknown-linux-musl ;;
  *) die "no release for $(uname -s)/$(uname -m); pass --hipc <path>" ;;
  esac
}

# Keeps the one SHA256SUMS line for this target, so the check needs no --ignore-missing,
# which an older shasum lacks; fails when there is no such line.
sums_list_tarball() {
  grep " hipc-$1.tar.gz\$" "$WORK/release/SHA256SUMS" >"$WORK/release/SHA256SUMS.target"
}

# Downloads the latest release into the work directory, verifies it and unpacks it, as the
# README tells a user to: cosign checks the signature of SHA256SUMS against the release
# workflow's identity for the release's tag, then shasum checks the tarball against its
# line. Each command is a step, so the walkthrough shows it.
install_release() {
  local target="<target>" tag="<tag>" sum
  if [[ $LIST == 0 ]]; then
    target="$(release_target)"
    mkdir -p "$WORK/release"
    cd "$WORK/release"
  fi
  step setup ok "Find the latest release" "its tag names the signing identity cosign checks" \
    -- gh release view --repo "$REPO" --json tagName --jq .tagName
  if [[ $LIST == 0 ]]; then
    tag="$(tr -d '[:space:]' <"$LAST_OUT")"
    # The tag goes into the identity cosign checks, so only a plain version tag is accepted.
    [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
      say "the latest release's tag is not vX.Y.Z; stopping"
      exit 1
    }
  fi
  step setup ok "Download the latest release" "a fresh install, as the README describes" \
    -- gh release download --repo "$REPO" "$tag" --pattern "hipc-$target.tar.gz" \
    --pattern SHA256SUMS --pattern SHA256SUMS.sigstore.json
  # A release made before the workflow signed SHA256SUMS has no bundle. The README treats
  # the signature as required, so the run stops here, with the reason, rather than check
  # the hash and go on, or fail inside cosign.
  if [[ $LIST == 0 && ! -s SHA256SUMS.sigstore.json ]]; then
    say "release $tag has no SHA256SUMS.sigstore.json: it predates signed checksums;" \
      "pass --hipc <path> to test a binary you verified yourself"
    exit 1
  fi
  check always "SHA256SUMS lists the tarball" -- sums_list_tarball "$target"
  step setup ok "Verify the signature" \
    "SHA256SUMS was signed by this repository's release workflow for this tag, and no other" \
    -- cosign verify-blob SHA256SUMS --bundle SHA256SUMS.sigstore.json \
    --certificate-identity "https://github.com/$REPO/.github/workflows/release.yml@refs/tags/$tag" \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com
  sum=(shasum -a 256 -c SHA256SUMS.target)
  [[ $LIST == 1 ]] || command -v shasum >/dev/null || sum=(sha256sum -c SHA256SUMS.target)
  step setup ok "Verify the checksum" "the tarball is the one the release published" -- "${sum[@]}"
  step setup ok "Unpack it" "the binary is hipc, at the top of the tarball" \
    -- tar -xzf "hipc-$target.tar.gz"
  if [[ $LIST == 0 ]]; then
    cd - >/dev/null
    HIPC="$WORK/release/hipc"
    [[ $FAILS == 0 && -x "$HIPC" ]] || {
      say "the release could not be installed; stopping"
      exit 1
    }
  fi
}

# The read-only commands whose answers make up the account snapshot: name, then argv. Every
# list that pages is read with --all. Public IPs have no account-wide list: they are read
# per VM, after `vms` (see take_snapshot).
SNAPSHOT_READS='buckets s3 buckets list
s3-tokens s3 tokens list
ssh-keys compute ssh-keys list --all
hub-keys hub keys list
hub-repositories hub repositories list
cdn-zones cdn zones list
cdn-lists cdn lists list
vms compute vms list
databases compute databases list
kubernetes-clusters compute kubernetes clusters list
network-peers network peers list
invites account invites list
tickets support tickets list --all
drive-subscription drive subscription get
s3-subscription s3 subscription get
hub-subscription hub subscription get
projects account projects list
billing-accounts account billing-accounts list
balance billing balance'

# What the snapshot keeps of an answer, so a second snapshot compares equal unless something
# was created, deleted, or changed plan:
# - a list: each live row's id, name, subscription_id, plan_id, active and cancelled_at,
#   sorted, so the API's order does not matter; sizes, timestamps and VM states are dropped.
#   A list that is not complete (see `complete`) is kept as {"incomplete": true}, a failure.
# - the balance: the `balance` field alone, compared on its own line.
# - a VM's public IP: its address, or null.
# - any other document: the fields that say which plan is active (active, status, plan,
#   plan_id, plan_code, mode, period, subscription_id, cancelled_at, canceled_at,
#   cancel_at_period_end), arrays sorted.
# - a ticket: its id, status and closed_at, so a ticket closed by mistake shows.
# - the Drive subscription: the whole answer, so the teardown's check that the plan this run
#   bought is cancelled reads what `drive subscription get` printed (drive_free reads every
#   key). The diff reduces it to the plan fields above and plan_name (compare_snapshots).
# shellcheck disable=SC2016 # jq source.
SNAPSHOT_FILTER='
def pick_doc:
  if type == "object" then
    with_entries(select(.key | IN("active", "status", "plan", "plan_id", "plan_code", "mode",
      "period", "subscription_id", "cancelled_at", "canceled_at", "cancel_at_period_end"))
      | .value |= pick_doc)
  elif type == "array" then map(pick_doc) | sort
  else . end;
if $name == "balance" then .balance
elif $name == "drive-subscription" then .
elif ($name | startswith("public-ip:")) then (.public_ip.address? // null)
elif rows != null then
  if complete | not then {incomplete: true}
  else
    [rows[] | select(revoked | not) |
     if type == "object" and $name == "tickets" then
       with_entries(select(.key | IN("id", "status", "closed_at")))
     elif type == "object" then
       with_entries(select(.key | IN("id", "vm_id", "name", "bucket_id", "bucket_name",
         "subscription_id", "plan_id", "active", "cancelled_at")))
     else . end] | sort
  end
else pick_doc end'

list_snapshot() {
  local name argv
  while read -r name argv; do
    printf '  [read] hipc --json %s\n' "$argv"
  done <<<"$SNAPSHOT_READS"
  printf '  [read] hipc --json compute public-ip get <vm> (for each VM)\n'
}

# Runs one snapshot read in the snapshot home and writes what the snapshot keeps to
# $WORK/snap.value. A read that fails, or a list that is not complete, fails the run:
# without it the two snapshots could compare equal while the account changed. Offline,
# every read fails with `unavailable`, as expected. Each read that gave no value, offline
# too, is named in $WORK/snap.unread, so the comparison of the two snapshots is not reported
# as a pass it did not make.
snapshot_read() {
  local name="$1" code
  shift
  if HOME="$WORK/snapshot/home" XDG_CONFIG_HOME="$WORK/snapshot/home/.config" \
    "$HIPC" --json "$@" >"$WORK/snap.out" 2>"$WORK/snap.err" </dev/null; then
    jq --arg name "$name" "$JQ_DEFS $SNAPSHOT_FILTER" "$WORK/snap.out" >"$WORK/snap.value" \
      2>/dev/null || printf '"not JSON"' >"$WORK/snap.value"
    if jq -e '. == "not JSON" or (type == "object" and .incomplete == true)' \
      "$WORK/snap.value" >/dev/null; then
      say "snapshot: $name: FAIL, the answer is not JSON or not a complete list"
      record FAIL "snapshot read $name"
      printf '%s\n' "$name" >>"$WORK/snap.unread"
    fi
    return 0
  fi
  code="$(error_code "$WORK/snap.err")"
  printf '{"error":"%s"}' "${code:-unknown}" >"$WORK/snap.value"
  if [[ "$name" == public-ip:* && "$code" == not_found ]]; then
    # The VM went away between the two reads; the VM list records that.
    return 0
  fi
  if [[ "$name" == drive-subscription && "$code" == not_found ]]; then
    # No Drive subscription at all: a state, as a cancelled one may become.
    return 0
  fi
  if [[ "$name" == network-peers || "$name" == kubernetes-clusters ]] &&
    [[ "$code" == not_found || "$code" == network_unprovisioned ]]; then
    # An account with no network yet, or no Kubernetes access: the error is the value, and
    # the two snapshots still compare equal. Creating the network would be a write.
    return 0
  fi
  if [[ "$name" == hub-keys || "$name" == hub-repositories ]] &&
    [[ "$code" == hub_unprovisioned ]]; then
    # An account with no Hub namespace: a state, as for the network. Creating one would be a
    # write.
    return 0
  fi
  printf '%s\n' "$name" >>"$WORK/snap.unread"
  if [[ $OFFLINE == 1 && "$code" == unavailable ]]; then
    return 0
  fi
  say "snapshot: $name: FAIL, the read failed: ${code:-unknown}"
  mask_secrets_in "$WORK/snap.err"
  show_file "$WORK/snap.err"
  record FAIL "snapshot read $name"
}

# Adds $WORK/snap.value to the snapshot file $1 under the key $2.
snapshot_put() {
  jq --arg k "$2" --slurpfile v "$WORK/snap.value" '.[$k] = $v[0]' "$1" >"$1.new"
  mv "$1.new" "$1"
}

# Writes the snapshot to $1 and prints it.
take_snapshot() {
  local out="$1" name argv vm
  local -a args vms=()
  printf '{}' >"$out"
  while read -r name argv; do
    # shellcheck disable=SC2206 # the argv words above are fixed and need splitting.
    args=($argv)
    snapshot_read "$name" "${args[@]}"
    snapshot_put "$out" "$name"
    if [[ "$name" == vms ]]; then
      jq -r "$JQ_DEFS"' rows // [] | .[] | objects | select(revoked | not)
        | (.vm_id // .id) // empty | tostring' "$WORK/snap.out" >"$WORK/snap.vms" \
        2>/dev/null || true
      while IFS= read -r vm; do
        [[ -z "$vm" ]] || vms+=("$vm")
      done <"$WORK/snap.vms"
    fi
  done <<<"$SNAPSHOT_READS"
  for vm in ${vms[@]+"${vms[@]}"}; do
    snapshot_read "public-ip:$vm" compute public-ip get "$vm"
    snapshot_put "$out" "public-ip:$vm"
  done
  jq -S . "$out" >"$out.new" && mv "$out.new" "$out"
  show_file "$out"
}

setup() {
  SC="setup"
  if [[ $LIST == 1 ]]; then
    printf 'scenario setup: install hipc, log in for the snapshot, snapshot the account\n'
    install_release
    step setup ok "Print the version" "the binary under test" -- hipc --output text version
    login
    list_snapshot
    return 0
  fi
  say "manual_test: tier $TIER, scenarios:$SELECTED"
  say "ledger: $WORK/ledger (after a SIGKILL, the list of what to delete by hand)"
  [[ $OFFLINE == 0 ]] || say "offline: every command runs with HIPPIUS_API_URL=$DEAD_ORIGIN and a fake token"
  if [[ -z "$HIPC" ]]; then
    install_release
  else
    say "hipc: $HIPC (from --hipc)"
  fi
  step setup ok "Print the version" "the binary under test" -- hipc --output text version
  if ! "$HIPC" schema </dev/null 2>/dev/null |
    jq -r '.errors[] | "\(.code) \(.exit)"' >"$WORK/exits" 2>/dev/null; then
    say "hipc schema gives no error table; every expected error code will FAIL"
    record FAIL "hipc schema gives no error table"
  fi
  # The script's own names for a network that was never set up and for an account with no Hub
  # namespace: each a 403, exit 1.
  printf 'network_unprovisioned 1\nhub_unprovisioned 1\n' >>"$WORK/exits"
  mkdir -p "$WORK/snapshot/home"
  H="$WORK/snapshot/home"
  enter_home "$H"
  login
  check_token
  say ""
  say "==== snapshot of the account before the first scenario"
  take_snapshot "$WORK/before.json"
  SNAPSHOT_TAKEN=1
}

# ---------------------------------------------------------------------------------------
# Teardown: the exit trap. It runs after the last scenario, after a failure, and after
# Ctrl-C, so the ledger is always worked off.

# Lists the resources of a ledger kind into $WORK/clean.out, in the snapshot home.
cleanup_list() {
  local -a args
  case "$1" in
  bucket) args=(s3 buckets list) ;;
  s3-token) args=(s3 tokens list) ;;
  ssh-key) args=(compute ssh-keys list --all) ;;
  hub-key) args=(hub keys list) ;;
  project) args=(account projects list) ;;
  cdn-list) args=(cdn lists list) ;;
  vm) args=(compute vms list) ;;
  database) args=(compute databases list) ;;
  cdn-zone) args=(cdn zones list) ;;
  esac
  "$HIPC" --json "${args[@]}" >"$WORK/clean.out" 2>"$WORK/clean.err" </dev/null
}

# The undo of a purchase that is not a delete: the Drive plan is cancelled, the ticket is
# closed, the public IP is released. Each is proved by a read afterwards, as a delete is by a
# list. $WORK/clean.out holds the read.
cleanup_state() {
  local kind="$1" name="$2" id="$3" state="$4" code
  local -a undo=() read=()
  case "$kind" in
  drive-sub)
    # Only a subscribe that exited 0 makes the line `created`. A `pending` line (the
    # subscribe was refused, failed, or was cut off) bought nothing this run can prove, and
    # a cancel would end a plan the account may already have paid for; the snapshot diff
    # shows any plan change. A `cancelled` line is one whose cancel already exited 0: it is
    # only read again, and never sent a second cancel.
    if [[ "$state" != created && "$state" != cancelled ]]; then
      say "cleanup: drive-sub $name ($state): the subscribe did not exit 0, so nothing is" \
        "cancelled; the snapshot diff shows whether the plan changed"
      return 0
    fi
    [[ "$state" == cancelled ]] || undo=(drive subscription cancel)
    read=(drive subscription get)
    ;;
  ticket)
    if [[ "$id" == - ]]; then
      say "cleanup: ticket $name ($state): no id; if one was opened, its subject asks the" \
        "support team to close it"
      return 0
    fi
    if ! has_command "support tickets update"; then
      say "cleanup: ticket $id: this hipc cannot close a ticket; its subject asks the support" \
        "team to close it"
      return 0
    fi
    undo=(support tickets update "$id" --status closed)
    read=(support tickets get "$id")
    ;;
  public-ip)
    if [[ "$id" == - ]]; then
      say "cleanup: public-ip on $name: the VM has no id; deleting the VM releases it"
      return 0
    fi
    undo=(compute public-ip delete "$id")
    read=(compute public-ip get "$id")
    ;;
  esac
  if [[ ${#undo[@]} -gt 0 ]]; then
    cleanup_undo "$kind" "${undo[@]}"
  else
    say "cleanup: $kind $name: the cancel exited 0 in the scenario; reading it again"
  fi
  if "$HIPC" --json "${read[@]}" >"$WORK/clean.out" 2>"$WORK/clean.err" </dev/null &&
    undone "$kind" "$WORK/clean.out"; then
    say "cleanup: $kind $name: undone"
    ledger_remove "$kind" "$name"
    return 0
  fi
  code="$(error_code "$WORK/clean.err")"
  if [[ $OFFLINE == 1 && "$code" == unavailable ]]; then
    say "cleanup: $kind $name: cannot read it (unavailable); offline, nothing was bought"
  elif [[ "$kind" == public-ip && "$code" == not_found ]]; then
    say "cleanup: $kind $name: released (not_found: the VM has no public IP, or is gone)"
    ledger_remove "$kind" "$name"
  elif [[ "$kind" == drive-sub && "$code" == not_found ]]; then
    say "cleanup: $kind $name: undone (not_found: no Drive subscription is left)"
    ledger_remove "$kind" "$name"
  else
    say "cleanup: $kind $name: FAIL, not provably undone; undo it by hand"
    record FAIL "cleanup: $kind $name may be left as bought"
  fi
}

# cleanup_undo <kind> <args...>: sends the undo, and shows its error when it fails. A Drive
# cancel is retried while it answers 425, as the scenario's is: the plan may still be
# confirming when the trap runs.
cleanup_undo() {
  local kind="$1" tries=0 retries=0
  shift
  [[ "$kind" != drive-sub ]] || retries=6
  say "cleanup: \$ hipc --json $(show_argv "$@") --yes"
  while ! "$HIPC" --json "$@" --yes >"$WORK/clean.out" 2>"$WORK/clean.err" </dev/null; do
    if [[ $tries -lt $retries ]] &&
      jq -e '.error | .http_status == 425 or
        ((.details // {}) | tostring | test("still_confirming"))' \
        "$WORK/clean.err" >/dev/null 2>&1; then
      tries=$((tries + 1))
      say "cleanup: 425: still confirming; try $((tries + 1)) of $((retries + 1))" \
        "in $RETRY_SECONDS seconds"
      sleep "$RETRY_SECONDS"
      continue
    fi
    mask_secrets_in "$WORK/clean.err"
    show_file "$WORK/clean.err"
    return 0
  done
}

# Puts back what a change step of kind $1 changed on the resource $2, from the value
# change_begin saved, and proves it by a read.
cleanup_restore() {
  local kind="$1" id="$2" value filter code hand=""
  local -a undo=() read=()
  value="$(cat "$WORK/restore/$kind.$id")"
  case "$kind" in
  runner-max)
    undo=(compute runners settings update --max-concurrent "$value")
    read=(compute runners settings get)
    filter="$RUNNER_MAX_IS"
    ;;
  runner-cap)
    printf '%s\n' "$value" >"$WORK/restore/runner-cap.body"
    undo=(api PUT /runners/limit/ --body-file "$WORK/restore/runner-cap.body")
    hand="printf '%s\\n' $(quote_word "$value") | hipc api PUT /runners/limit/ --body-file - --yes"
    read=(compute runners limit get)
    filter="$RUNNER_CAP_BACK"
    ;;
  billing-name)
    undo=(account billing-accounts update "$id" --name "$value")
    read=(account billing-accounts list)
    filter="$BILLING_NAME_IS"
    ;;
  esac
  cleanup_undo "$kind" "${undo[@]}"
  if "$HIPC" --json "${read[@]}" >"$WORK/clean.out" 2>"$WORK/clean.err" </dev/null &&
    jq -e --arg id "$id" --arg v "$value" "$JQ_DEFS $filter" "$WORK/clean.out" >/dev/null; then
    say "cleanup: $kind $id: put back"
    ledger_remove "$kind" "$id"
    return 0
  fi
  code="$(error_code "$WORK/clean.err")"
  if [[ $OFFLINE == 1 && "$code" == unavailable ]]; then
    say "cleanup: $kind $id: cannot read it (unavailable); offline, nothing was changed"
  else
    [[ -n "$hand" ]] || hand="hipc $(show_argv "${undo[@]}") --yes"
    say "cleanup: $kind $id: FAIL, not provably put back; put it back by hand: $hand"
    record FAIL "cleanup: $kind $id may be left changed"
  fi
}

# What a change step's read must show once the value is back: $v is the value change_begin
# saved, $id the resource.
# shellcheck disable=SC2016 # jq source.
RUNNER_MAX_IS='(.max_concurrent | tostring) == $v'
# shellcheck disable=SC2016 # jq source.
# A cap is compared as a number, so `21` and `21.000000000000000000` are the same cap.
RUNNER_CAP_IS='((.monthly_cap_credits | tostring | tonumber?) // null) == ($v | tonumber)'
# The cap is back when the server's own `monthly_cap` is what it was.
# shellcheck disable=SC2016 # jq source.
RUNNER_CAP_BACK='{monthly_cap} == ($v | fromjson)'
# shellcheck disable=SC2016 # jq source.
BILLING_NAME_IS='any(rows // [] | .[] | objects; (.id | tostring) == $id and .name == $v)'

# Whether the read in $2 shows the purchase of kind $1 undone.
undone() {
  case "$1" in
  drive-sub) jq -e "$DRIVE_CANCELLED"' drive_cancelled' "$2" >/dev/null ;;
  ticket) jq -e '(.status // "") | tostring | test("closed"; "i")' "$2" >/dev/null ;;
  public-ip) jq -e '(.public_ip.address? // .address? // null) == null' "$2" >/dev/null ;;
  esac
}

# Works off one ledger line. A line with the id this run's create answered is deleted by
# that id; a line without one (the create failed, timed out, or was interrupted) is looked
# up by its name, which is unique to this run. Either way the result counts only when a
# complete list then no longer shows the name; anything less is a FAIL, and the resource
# is named so it can be deleted by hand.
cleanup_one() {
  local kind="$1" name="$2" id="$3" state="$4" code
  local -a delete_args
  case "$kind" in
  drive-sub | ticket | public-ip)
    cleanup_state "$@"
    return 0
    ;;
  runner-max | runner-cap | billing-name)
    cleanup_restore "$kind" "$name"
    return 0
    ;;
  esac
  if [[ "$id" == - ]]; then
    if ! cleanup_list "$kind"; then
      code="$(error_code "$WORK/clean.err")"
      if [[ $OFFLINE == 1 ]]; then
        say "cleanup: $kind $name: cannot list (${code:-unknown}); offline, nothing was created"
      else
        say "cleanup: $kind $name ($state): FAIL, cannot list to look it up: ${code:-unknown}"
        record FAIL "cleanup: $kind $name could not be looked up"
      fi
      return 0
    fi
    id="$(jq -r --arg n "$name" "$JQ_DEFS"' first(rows // [] | .[] | objects
      | select(row_name == $n and (revoked | not)) | row_id) // empty' \
      "$WORK/clean.out")"
    if [[ -z "$id" ]]; then
      if jq -e "$JQ_DEFS complete" "$WORK/clean.out" >/dev/null; then
        say "cleanup: $kind $name ($state): not on the server, nothing to delete"
        ledger_remove "$kind" "$name"
      else
        say "cleanup: $kind $name ($state): FAIL, not on the first page and the list is not complete"
        record FAIL "cleanup: cannot prove $kind $name is gone"
      fi
      return 0
    fi
  fi
  case "$kind" in
  bucket) delete_args=(s3 buckets delete "$name") ;;
  s3-token) delete_args=(s3 tokens revoke --kind sub "$id") ;;
  ssh-key) delete_args=(compute ssh-keys delete "$id") ;;
  hub-key) delete_args=(hub keys delete "$id") ;;
  project) delete_args=(account projects delete "$id") ;;
  cdn-list) delete_args=(cdn lists delete "$name") ;;
  vm) delete_args=(compute vms delete "$id") ;;
  database) delete_args=(compute databases delete "$id") ;;
  cdn-zone) delete_args=(cdn zones delete "$id") ;;
  esac
  say "cleanup: \$ hipc --json $(show_argv "${delete_args[@]}") --yes"
  if ! "$HIPC" --json "${delete_args[@]}" --yes >"$WORK/clean.out" 2>"$WORK/clean.err" \
    </dev/null; then
    mask_secrets_in "$WORK/clean.err"
    show_file "$WORK/clean.err"
  fi
  if cleanup_list "$kind" &&
    jq -e --arg n "$name" "$JQ_DEFS"' complete and (live_names | any(.[]; . == $n) | not)' \
      "$WORK/clean.out" >/dev/null; then
    say "cleanup: $kind $name: deleted"
    ledger_remove "$kind" "$name"
  else
    say "cleanup: $kind $name: FAIL, still on the server or not provably gone; delete it by hand"
    record FAIL "cleanup: $kind $name may be left on the server"
  fi
}

run_cleanup() {
  local -a lines=()
  local line i
  say ""
  say "==== cleanup ledger"
  while IFS= read -r line; do
    [[ -z "$line" ]] || lines+=("$line")
  done <"$WORK/ledger"
  if [[ ${#lines[@]} -eq 0 ]]; then
    say "cleanup: the ledger is empty; every scenario deleted what it created"
    return 0
  fi
  # Newest first: a token goes before the bucket it is scoped to.
  for ((i = ${#lines[@]} - 1; i >= 0; i--)); do
    # shellcheck disable=SC2086 # a ledger line is four words: kind, name, id and state.
    cleanup_one ${lines[i]}
  done
}

# The snapshots without the balance must be equal. The balance is compared on its own line:
# a VM or a metered service that runs during the test moves it without this run's doing.
#
# drive_free: the Drive subscription read shows, positively, that no paid plan is in use, so
# a subscribe changes no plan the account pays for: `plan` and `plan_code`, each absent or
# null, with at least one the string `free`, and a `plan_id` absent, null or `free`; or no
# plan at all: `{"active": false}`, with `free_plan` (the API documents it as whether the
# account falls back to Free; wallet-only accounts do not) and no other key but a plan code,
# name, id, subscription id or status that is null; or the read's `not_found`. Any other
# shape is taken for a paid plan: a nested plan, a plan name beside a null plan, and an
# inactive answer that still carries a status (`past_due`), a period end or a funding source.
# drive_cancelled: the shapes a Drive subscription may have after this run cancelled the
# plan it bought: a cancelled status (`cancelled`, `canceled`, `cancel_submitted`), a cancel
# at the period's end, or a free state as above. A cancel date alone is not one: an active,
# paid plan can carry the date of an earlier cancel that was undone.
# shellcheck disable=SC2016 # jq source.
DRIVE_CANCELLED='
def drive_free:
  type == "object" and
  (if .error != null then .error == "not_found"
   else
     [.plan, .plan_code] as $named
     | if any($named[]; . != null) then
         all($named[]; . == null or (type == "string" and ascii_downcase == "free")) and
         (.plan_id == null or ((.plan_id | type) == "string" and
           (.plan_id | ascii_downcase) == "free"))
       else
         .active == false and
         all(to_entries[]; .key == "active" or .key == "free_plan" or
           (.value == null and (.key | IN("plan", "plan_code", "plan_name", "plan_id",
             "subscription_id", "status"))))
       end
   end);
def drive_cancelled:
  type == "object" and
  (((.status | type) == "string" and
    (.status | ascii_downcase | IN("cancelled", "canceled", "cancel_submitted"))) or
   .cancel_at_period_end == true or drive_free);
'

# The backup bucket and writer token the server made for a database this run bought, and
# left after its delete: a FAIL of its own, named, and written to $WORK/left for the kept
# ledger. The script deletes neither: the owner decides.
server_left() {
  local made
  made="$(jq -c --slurpfile dbs "$WORK/dbs.json" "$JQ_DEFS $1" "$WORK/after.json" 2>/dev/null)"
  [[ -n "$made" ]] || made='{"buckets":[],"tokens":[]}'
  # shellcheck disable=SC2016 # jq source.
  jq -r "$JQ_DEFS"' (.buckets[] | "bucket \(row_name) \(.bucket_id // "-") left"),
    (.tokens[] | "s3-token \(row_name) \(.id // "-") left")' <<<"$made" >"$WORK/left"
  [[ -s "$WORK/left" ]] || return 0
  say "check: the server left the database's backup bucket and its writer token after the" \
    "delete: FAIL; they are $(awk '{ print $1 " " $2 }' "$WORK/left" | paste -sd , -)," \
    "listed in the kept ledger; the script deletes neither"
  record FAIL "the server left the database's backup bucket and its writer token"
}

compare_snapshots() {
  local before after
  say ""
  say "==== snapshot of the account after the last scenario"
  take_snapshot "$WORK/after.json"
  # The spend tier's own differences, and only those, are allowed: the Drive subscription
  # when this run bought one, but only in a cancelled shape (checked below), and the rows of
  # the tickets this run opened. Anything else in those two keys still shows in the diff.
  local drive=false
  ! grep -qx 'drive-sub' "$WORK/spent" || drive=true
  awk '$1 == "ticket" && $2 != "-" { print $2 }' "$WORK/spent" | jq -R . | jq -s . \
    >"$WORK/opened.json"
  # A database this run bought gets a backup bucket and a writer token from the server,
  # which its delete leaves; they are reported apart below, not as a difference. A VM launch
  # sets up the account's private network, so the peers read may go from
  # network_unprovisioned to an empty list.
  awk '$1 == "database" { print $2 }' "$WORK/spent" | jq -R . | jq -s . >"$WORK/dbs.json"
  local vm=false
  ! grep -qx 'vm' "$WORK/spent" || vm=true
  # shellcheck disable=SC2016 # jq source.
  local server_made='($dbs[0] | map("hippius-db-\(.)-backups")) as $buckets
    | ($dbs[0] | map("db-\(.)-backup-writer")) as $tokens
    | {buckets: [.buckets | arrays | .[] | select(row_name | IN($buckets[]))],
       tokens: [."s3-tokens" | arrays | .[] | select(row_name | IN($tokens[]))]}'
  # shellcheck disable=SC2016 # jq source.
  local allow="$DRIVE_CANCELLED $JQ_DEFS"'
    ('"$server_made"') as $made
    | del(.balance)
    | if (.buckets | type) == "array" then .buckets -= $made.buckets else . end
    | if (."s3-tokens" | type) == "array" then ."s3-tokens" -= $made.tokens else . end
    | if $vm and $before[0]."network-peers" == {"error": "network_unprovisioned"} and
        $after[0]."network-peers" == [] then
        ."network-peers" = "allowed: set up by the VM launch"
      else . end
    | if (."drive-subscription" | type) == "object" then
        ."drive-subscription" |= with_entries(select(.key | IN("active", "status", "plan",
          "plan_id", "plan_code", "plan_name", "period", "subscription_id", "cancelled_at",
          "canceled_at", "cancel_at_period_end", "error")))
      else . end
    | if $drive and ($after[0]."drive-subscription" | drive_cancelled) then
        ."drive-subscription" = "allowed: cancelled by this run"
      else . end
    | if (.tickets | type) == "array" then
        .tickets |= map(select(((.id // "") | tostring) as $i | $opened[0] | index($i) | not))
      else . end'
  local -a args=(--argjson drive "$drive" --argjson vm "$vm" --slurpfile opened
    "$WORK/opened.json" --slurpfile dbs "$WORK/dbs.json" --slurpfile before "$WORK/before.json"
    --slurpfile after "$WORK/after.json")
  jq "${args[@]}" "$allow" "$WORK/before.json" >"$WORK/before.rest"
  jq "${args[@]}" "$allow" "$WORK/after.json" >"$WORK/after.rest"
  if [[ $vm == true ]] && jq -e '."network-peers" == "allowed: set up by the VM launch"' \
    "$WORK/after.rest" >/dev/null; then
    say "note: the VM launch set up the account's private network (the peers read went from" \
      "network_unprovisioned to no peers); the spend tier allows exactly that change"
  fi
  server_left "$server_made"
  if [[ "$drive" == true ]]; then
    if [[ $OFFLINE == 1 ]]; then
      say "check: the Drive plan this run bought is cancelled: SKIP (offline)"
      record SKIP "the Drive plan is cancelled"
    elif jq -e "$DRIVE_CANCELLED"' ."drive-subscription" | drive_cancelled' \
      "$WORK/after.json" >/dev/null; then
      say "check: the Drive plan this run bought is cancelled: PASS"
      record PASS "the Drive plan is cancelled"
    else
      say "check: the Drive plan this run bought is cancelled: FAIL; cancel it by hand"
      record FAIL "the Drive plan this run bought is not cancelled"
    fi
  fi
  # Two snapshots that each failed the same read compare equal and prove nothing, so the
  # check passes only when every read gave a value; a failed read already failed the run,
  # except offline, where every read fails as expected.
  local unread=""
  if [[ -s "$WORK/snap.unread" ]]; then
    unread="$(sort -u "$WORK/snap.unread" | paste -sd ' ' -)"
  fi
  if [[ -n "$unread" ]]; then
    say "check: the account is as it was found: SKIP (not read: $unread)"
    record SKIP "the account is as it was found"
  elif diff -u "$WORK/before.rest" "$WORK/after.rest" >"$WORK/snapshot.diff"; then
    say "check: the account is as it was found: PASS"
    record PASS "the account is as it was found"
  else
    say "the snapshots differ:"
    show_file "$WORK/snapshot.diff"
    say "check: the account is as it was found: FAIL"
    record FAIL "the account differs from the snapshot taken before the first scenario"
  fi
  before="$(jq -c '.balance' "$WORK/before.json")"
  after="$(jq -c '.balance' "$WORK/after.json")"
  # A balance is a number or a decimal string. A failed read (an `error` object) or an answer
  # without `balance` (null) on both sides would compare equal, so it is no pass: offline,
  # where the read fails as expected, the check is skipped, and online it fails.
  if ! jq -e '.balance | type == "string" or type == "number"' "$WORK/before.json" \
    >/dev/null 2>&1 ||
    ! jq -e '.balance | type == "string" or type == "number"' "$WORK/after.json" \
      >/dev/null 2>&1; then
    if [[ $OFFLINE == 1 ]]; then
      say "check: the balance is unchanged: SKIP (no balance was read: $before, then $after)"
      record SKIP "the balance is unchanged"
    else
      say "check: the balance is unchanged: FAIL (no balance was read: $before, then $after)"
      record FAIL "no balance was read, so the balance check proves nothing"
    fi
  elif [[ "$before" == "$after" ]]; then
    say "check: the balance is unchanged ($before): PASS"
    record PASS "the balance is unchanged"
  elif [[ $SPENT == 1 ]]; then
    say "check: the balance moved from $before to $after, as the purchases this run made" \
      "spend it: PASS"
    record PASS "the balance moved by the purchases"
  else
    say "check: the balance moved from $before to $after: FAIL (nothing this run does is billed; a running VM or a metered service can move it, so check by hand)"
    record FAIL "the balance moved from $before to $after"
  fi
}

# The log must not hold the token or a secret a create revealed. grep reads them from a
# file, so they are not in its argv. Only a count is printed.
#
# Besides what the mask learned as the run went, every step's raw stdout and stderr is read
# again for its secret fields, so a secret the mask missed is still found.
#
# The work is done by jq, awk and grep, each over whole files: no bash loop reads a value or
# a line, and there is no process substitution. bash 3.2, the macOS /bin/bash, spun on the
# CPU for minutes in the loops this replaced (a read loop over a process substitution per
# step file), at the end of a whole offline run.
scan_log() {
  local hits file started=$SECONDS
  # needles.jsonl: one {"v": needle, "m": where it came from} per line. First what the mask
  # learned as the run went, with the step it learned it at.
  jq -n -c -R --rawfile src "$WORK/secrets.src" '($src | split("\n")) as $m
    | [inputs] | to_entries[] | {v: .value, m: $m[.key]}' "$WORK/secrets" \
    >"$WORK/needles.jsonl"
  # Then each secret field of each step's raw output: jq stops at the first file that is not
  # JSON, so each file gets its own run, and an empty one (most stderr) holds nothing.
  for file in "$WORK"/steps/*; do
    [[ -s "$file" ]] || continue
    jq -c --arg f "${file##*/}" "$SECRET_ENTRIES"' | {k: .key, v: .value, f: $f}' "$file" \
      2>/dev/null || true
  done >"$WORK/secrets.raw"
  # Each value with its trailing line breaks dropped, as every line it holds and as its JSON
  # escape: grep -F takes each line of a file as one needle.
  jq -c '(.v | sub("\n+$"; "")) as $v | "step file \(.f), field \(.k)" as $at
    | ($v | split("\n")) as $lines
    | if ($lines | length) > 1
      then ($lines[] | {v: ., m: "\($at), a line of the value"})
      else {v: $v, m: "\($at), the whole value"} end,
      {v: ($v | tojson | .[1:-1]), m: "\($at), its JSON escape"}' "$WORK/secrets.raw" \
    >>"$WORK/needles.jsonl" 2>/dev/null || true
  # An empty needle would match every line.
  jq -r 'select(.v != "") | .v' "$WORK/needles.jsonl" >"$WORK/needles.txt"
  jq -r 'select(.v != "") | .m' "$WORK/needles.jsonl" >"$WORK/needles.src"
  # A mask that replaced a short secret inside a longer one first would leave the longer
  # one's other parts in the log, and the scan for the whole of it would pass. So the parts
  # of each secret around every shorter secret it contains are needles too, from 8 bytes up:
  # a shorter part (`-2026`, `abcd`) turns up in a bucket name, a timestamp, or a UUID as
  # often as in a leak. awk reads the needles from the file, so none is in its argv.
  # shellcheck disable=SC2016 # awk source.
  awk -v parts="$WORK/needles.parts.src" 'NR == FNR { m[FNR] = $0; next }
    { s[FNR] = $0; n = FNR }
    END {
      for (i = 1; i <= n; i++) for (j = 1; j <= n; j++) {
        if (length(s[j]) >= length(s[i]) || index(s[i], s[j]) == 0) continue
        rest = s[i]
        while ((k = index(rest, s[j])) > 0) {
          if (k > 8) part(substr(rest, 1, k - 1), i, j)
          rest = substr(rest, k + length(s[j]))
        }
        if (length(rest) >= 8) part(rest, i, j)
      }
    }
    function part(p, i, j) {
      print p
      print "a part of needle " i " (" m[i] ") around needle " j " (" m[j] ")" >parts
    }' "$WORK/needles.src" "$WORK/needles.txt" >"$WORK/needles.parts"
  cat "$WORK/needles.parts" >>"$WORK/needles.txt"
  cat "$WORK/needles.parts.src" >>"$WORK/needles.src" 2>/dev/null || true
  hits="$(grep -cF -f "$WORK/needles.txt" -- "$LOG" || true)"
  say "scan: the log scan took $((SECONDS - started)) seconds"
  if [[ "$hits" == 0 ]]; then
    say "check: the log holds neither the token nor a revealed secret: PASS"
    record PASS "log scan"
  else
    say "check: the log holds the token or a revealed secret on $hits lines: FAIL"
    record FAIL "the log holds the token or a revealed secret"
    scan_report
  fi
}

# Says, for each needle the log holds, where the needle came from, how long it is, and the
# numbers of the log lines that hold it: never the needle, nor the lines. The report is
# written to the log after the scan, so its own lines are not counted.
scan_report() {
  # shellcheck disable=SC2016 # awk source.
  awk -v srcs="$WORK/needles.src" -v log_file="$LOG" 'BEGIN {
      while ((getline line < srcs) > 0) m[++ns] = line
    }
    { s[FNR] = $0; n = FNR }
    END {
      while ((getline line < log_file) > 0) {
        ln++
        for (i = 1; i <= n; i++) if (index(line, s[i]) > 0) {
          if (count[i] < 10) at[i] = at[i] (count[i] ? "," : "") ln
          count[i]++
        }
      }
      for (i = 1; i <= n; i++) if (count[i]) printf "scan: needle %d (%s, %d bytes) is on " \
        "%d log lines: %s%s\n", i, m[i], length(s[i]), count[i], at[i],
        (count[i] > 10 ? ",..." : "")
    }' "$WORK/needles.txt" >"$WORK/scan.report"
  show_file "$WORK/scan.report"
}

# hipc must print a cluster key file's private keys nowhere, so the raw stdout and stderr of
# every step are scanned for the test key's seeds, before the mask, as release-check.sh
# scans its outputs. The log is scanned too, in case a line reached it some other way.
scan_key_outputs() {
  local hits
  hits="$(grep -lF -e "$TEST_KEY_SIGN_SEED" -e "$TEST_KEY_BOX_SEED" -- "$WORK"/steps/* "$LOG" \
    2>/dev/null | wc -l | tr -d ' ' || true)"
  if [[ "$hits" == 0 ]]; then
    say "check: no step output holds the cluster key file's secrets: PASS"
    record PASS "key scan"
  else
    say "check: the cluster key file's secrets reached $hits output files: FAIL"
    record FAIL "a step printed the cluster key file's secrets"
  fi
}

# Copies the ledger to $1 for the owner to work off by hand: first what may be left on the
# server, then, under a heading and commented out, the lines that name nothing to delete (a
# Drive subscribe that never exited 0, a ticket with no id), then, under their own heading,
# what the server made for a purchase and kept after its delete (server_left).
keep_ledger() {
  # shellcheck disable=SC2016 # awk source: $1, $3 and $4 are awk fields.
  awk '($1 == "drive-sub" && $4 == "pending") || ($1 == "ticket" && $3 == "-") {
      info[++n] = $0
      next
    }
    { print }
    END {
      if (n) {
        print ""
        print "# nothing to delete, for information: a purchase that never exited 0"
        for (i = 1; i <= n; i++) print "# " info[i]
      }
    }' "$WORK/ledger" >"$1"
  if [[ -s "$WORK/left" ]]; then
    {
      printf '\n# left by the server: made for a purchase and kept after its delete; the owner\n'
      printf '# decides whether to delete them\n'
      cat "$WORK/left"
    } >>"$1"
  fi
}

finish() {
  local status=$?
  set +e
  trap '' INT TERM
  if [[ -n "$WORK" && -d "$WORK" ]]; then
    if [[ $status == 130 || $status == 143 ]]; then
      say ""
      say "interrupted; cleaning up"
      record FAIL "the run was interrupted"
    fi
    if [[ $SNAPSHOT_TAKEN == 1 ]]; then
      H="$WORK/snapshot/home"
      enter_home "$H"
      SC="teardown"
      run_cleanup
      compare_snapshots
      logout
    fi
    say ""
    scan_log
    scan_key_outputs
    say ""
    say "result: $PASSES passed, $FAILS failed, $SKIPS skipped"
    if [[ -s "$WORK/failures" ]]; then
      say "failures:"
      show_file "$WORK/failures"
    fi
    say "log: $LOG"
    # The ledger names what a failed cleanup may have left on the server, and holds no
    # secret, so it outlives the work directory when a cleanup failed. The trap removed the
    # lines it cleaned, so the copy lists only what is left.
    if grep -q ': cleanup: ' "$WORK/failures" 2>/dev/null || [[ -s "$WORK/left" ]]; then
      keep_ledger "$LOG.ledger"
      say "ledger: $LOG.ledger (what may be left on the account: see its headings)"
    fi
    # $WORK holds secrets: the stored token, the token file, and the S3 and Hub secrets the
    # write tier saves. It is removed with rm, not moved to a trash folder, which would keep
    # every file it receives (scripts/release-check.sh does the same, for the same reason).
    rm -r -- "$WORK"
    [[ $FAILS == 0 || $status != 0 ]] || status=1
  fi
  exit "$status"
}

# ---------------------------------------------------------------------------------------
# Main.

SNAPSHOT_TAKEN=0
SUFFIX="$(date +%Y%m%d%H%M%S)-$$"
BUCKET="hipc-manual-$SUFFIX"
TOKEN_NAME="hipc-manual-$SUFFIX"
HUB_KEY="hipc-manual-$SUFFIX"
SSH_KEY="hipc-manual-$SUFFIX"
ZONE_NAME="hipc-manual-$SUFFIX"
PROJECT_NAME="hipc-manual-$SUFFIX"
# The name a write tier rename gives a peer or a billing account before it puts the old back.
RENAME="hipc-manual-$SUFFIX"
# A WAF list name is 1 to 32 of a-z, 0-9 and _.
WAF_LIST="hipc_manual_${SUFFIX//-/_}"
WAF_LIST="${WAF_LIST:0:32}"
VM_NAME="hipc-manual-$SUFFIX"
DB_NAME="hipc-manual-$SUFFIX"
CDN_BUCKET="hipc-manual-cdn-$SUFFIX"
TICKET_TAG="hipc-manual-$SUFFIX"

run_scenarios() {
  local name
  for name in $SELECTED; do
    case "$name" in
    first-run) scenario_first_run ;;
    account-billing) scenario_account_billing ;;
    drive) scenario_drive ;;
    s3) scenario_s3 "$BUCKET" "$TOKEN_NAME" ;;
    hub) scenario_hub "$HUB_KEY" ;;
    compute) scenario_compute "$SSH_KEY" "$VM_NAME" "$DB_NAME" ;;
    cdn) scenario_cdn "$BUCKET" "$ZONE_NAME" "$WAF_LIST" "$CDN_BUCKET" ;;
    network-kubernetes) scenario_network_kubernetes ;;
    team) scenario_team "$PROJECT_NAME" ;;
    support) scenario_support "$TICKET_TAG" ;;
    agent) scenario_agent "$BUCKET" ;;
    token-hygiene) scenario_token_hygiene ;;
    esac
  done
}

list_teardown() {
  printf '\nscenario teardown: work off the ledger, snapshot again, diff, log out, scan the log\n'
  printf '  [check] every ledger entry is deleted\n'
  list_snapshot
  [[ $SPENDS == 0 ]] || printf '  [check] the Drive plan this run bought is cancelled\n'
  printf '  [check] the account is as it was found\n'
  if [[ $SPENDS == 0 ]]; then
    printf '  [check] the balance is unchanged\n'
  else
    printf '  [check] the balance is unchanged, or moved by the purchases\n'
  fi
  logout
  printf '  [check] the log holds neither the token nor a revealed secret\n'
  printf '  [check] no step output holds the cluster key file'"'"'s secrets\n'
}

# The commands no tier runs, as itself or as a --dry-run: `path: reason`, one per line.
# `--excluded` prints them, and a test fails when a command of `hipc schema` is neither run
# nor named here, or is named here and run too.
EXCLUDED='completions: prints a shell script; the binary is covered by its tests, not the account
billing gamma get: the API has gamma payments off until gamma is enabled on subnet 75, and a run needs a funded Bittensor hotkey
billing gamma hotkeys create: the API has gamma payments off until gamma is enabled on subnet 75, and a run needs a funded Bittensor hotkey
billing gamma hotkeys delete: the API has gamma payments off until gamma is enabled on subnet 75, and a run needs a funded Bittensor hotkey
billing gamma conversions create: the API has gamma payments off until gamma is enabled on subnet 75, and a run needs a funded Bittensor hotkey
billing gamma conversions list: the API has gamma payments off until gamma is enabled on subnet 75, and a run needs a funded Bittensor hotkey
billing gamma conversions get: the API has gamma payments off until gamma is enabled on subnet 75, and a run needs a funded Bittensor hotkey'

if [[ $EXCLUDED_ONLY == 1 ]]; then
  printf '%s\n' "$EXCLUDED"
  exit 0
fi

if [[ $LIST == 1 ]]; then
  [[ -n "$HIPC" ]] || HIPC="hipc"
  BUCKET="<bucket>" TOKEN_NAME="<token-name>" HUB_KEY="<hub-key>" SSH_KEY="<ssh-key>"
  ZONE_NAME="<zone-name>" PROJECT_NAME="<project-name>" WAF_LIST="<waf-list>"
  VM_NAME="<vm-name>" DB_NAME="<database-name>" CDN_BUCKET="<cdn-bucket>"
  TICKET_TAG="<ticket-tag>" RENAME="<new-name>"
  setup
  run_scenarios
  list_teardown
  exit 0
fi

command -v jq >/dev/null || die "jq is required"
# The xtrace refusal at the top covers `bash -x` and SHELLOPTS; this covers a `set -x` added
# while editing the script.
set +x
if [[ $OFFLINE == 1 ]]; then
  TOKEN="$OFFLINE_TOKEN"
else
  TOKEN="${HIPPIUS_TOKEN:-}"
  # Trimmed as hipc trims it, so the log scan looks for the bytes hipc would print.
  TOKEN="${TOKEN#"${TOKEN%%[![:space:]]*}"}"
  TOKEN="${TOKEN%"${TOKEN##*[![:space:]]}"}"
  [[ -n "$TOKEN" ]] || die "set HIPPIUS_TOKEN to a console API token, or pass --offline"
  [[ "$TOKEN" != *[[:space:]]* ]] || die "HIPPIUS_TOKEN holds whitespace inside it"
fi
# The token stays in this shell only: hipc reads it from the profile that `auth login` stores
# in each temporary home, so no hipc process gets it from the environment or argv. Every
# HIPPIUS_, AWS_ and DOCKER_ variable goes too, so the caller's credentials, profiles and
# role settings cannot leak into a scenario.
unset CI AI_AGENT
while IFS= read -r name; do
  case "$name" in
  HIPPIUS_* | AWS_* | DOCKER_*) unset "$name" ;;
  esac
done < <(compgen -e)
# Offline, the bad-token checks set HIPPIUS_TOKEN for the dead origin, which it is bound to
# here: without HIPPIUS_TOKEN_ORIGIN, hipc sends HIPPIUS_TOKEN to the default origin only.
if [[ $OFFLINE == 1 ]]; then
  # Something listening on the dead origin would receive the fake token and every request;
  # bash's /dev/tcp connects and sends nothing, so the run checks first. Once: a listener
  # started during the run is not caught. The tests' port is held (Linux) or freshly
  # checked (macOS).
  if (exec 3<>"/dev/tcp/127.0.0.1/${DEAD_ORIGIN##*:}") 2>/dev/null; then
    die "something listens on ${DEAD_ORIGIN#http://}, the origin --offline needs closed;" \
      "stop it and run again"
  fi
  export HIPPIUS_API_URL="$DEAD_ORIGIN" HIPPIUS_TOKEN_ORIGIN="$DEAD_ORIGIN"
fi
if [[ -n "$HIPC" ]]; then
  [[ -x "$HIPC" && ! -d "$HIPC" ]] || die "--hipc $HIPC is not an executable file"
  [[ "$HIPC" == /* ]] || HIPC="$PWD/$HIPC"
else
  command -v gh >/dev/null || die "gh is required to download the release; or pass --hipc <path>"
  # The README treats the signature as required, and names cosign 3.0 or later, so the
  # install is not attempted without one. `cosign version` prints a `GitVersion: vX.Y.Z`
  # line in every release; an output without one is refused too, rather than guessed at.
  command -v cosign >/dev/null ||
    die "cosign is required to verify the release's signature;" \
      "install cosign 3.0 or later, or pass --hipc <path>"
  COSIGN_MAJOR="$(cosign version 2>&1 |
    sed -n 's/^GitVersion:[[:space:]]*v\{0,1\}\([0-9][0-9]*\)\..*/\1/p' | head -n 1)"
  [[ "$COSIGN_MAJOR" =~ ^[0-9]+$ && "$COSIGN_MAJOR" -ge 3 ]] ||
    die "cosign 3.0 or later is required to verify the release's signature" \
      "(cosign version reports ${COSIGN_MAJOR:-no GitVersion}); upgrade it, or pass --hipc <path>"
fi
[[ "$LOG" == /* ]] || LOG="$PWD/$LOG"
: >"$LOG" || die "cannot write the log $LOG"
# umask covers a new log; an existing one keeps its mode unless it is set here.
chmod 600 "$LOG"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/hipc-manual.XXXXXX")"
mkdir -p "$WORK/steps" "$WORK/scenarios" "$WORK/restore"
: >"$WORK/ledger"
: >"$WORK/secrets"
: >"$WORK/secrets.src"
: >"$WORK/failures"
# What the spend tier bought and expects the snapshot to show: `drive-sub`, `ticket <id>`.
: >"$WORK/spent"
: >"$WORK/left"
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

printf '%s\n' "$TOKEN" >"$WORK/token"
# The mask, applied with `sed -E -f` to every printed line: the token preview hipc prints
# (its last four characters), the token, each secret a step reveals (see add_pattern), and
# the value of a `password`, `passphrase`, `pass`, `pwd`, `secret` or `token` in any query
# string, in any case.
cat >"$WORK/mask.head" <<'EOF'
s/("token_preview"[[:space:]]*:[[:space:]]*")([^"\\]|\\.)*"/\1****"/g
s/^(token_preview[[:space:]]+).*$/\1****/
EOF
cat >"$WORK/mask.tail" <<'EOF'
s/([?&;]([Pp][Aa][Ss][Ss]([Ww][Oo][Rr][Dd]|[Pp][Hh][Rr][Aa][Ss][Ee])?|[Pp][Ww][Dd]|[Ss][Ee][Cc][Rr][Ee][Tt]|[Tt][Oo][Kk][Ee][Nn])=)[^&#;"'[:space:]]*/\1<redacted>/g
EOF
: >"$WORK/mask.rules"
build_mask
add_secret "$TOKEN" "<token>"
# The test cluster key's seeds, so the mask and the log scan know them from the start.
add_secret "$TEST_KEY_SIGN_SEED" "<cluster-key>"
add_secret "$TEST_KEY_BOX_SEED" "<cluster-key>"
# The stand-in value of the App Engine launch's secrets file; the launch is only a dry run.
APP_SECRET="hipc-manual-app-secret-$SUFFIX-$RANDOM"
printf '%s\n' "$APP_SECRET" >"$WORK/app-secret"
add_secret "$APP_SECRET" "<app-secret>"

setup
run_scenarios
