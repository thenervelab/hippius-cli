# shellcheck shell=bash
# Helpers that scripts/release-check.sh and scripts/manual_test.sh share. Sourced, never
# run: it defines functions and constants and changes no shell option.
#
# Written for bash 3.2, the macOS /bin/bash, as both scripts are.

# refuse_xtrace <script name>: exits 2 when the shell traces its commands. Tracing would
# print the token the moment it is read, and every secret after it, so each script calls
# this before it reads one.
refuse_xtrace() {
  case "$-" in
  *x*)
    echo "$1: refusing to run with xtrace (bash -x or SHELLOPTS=xtrace): it prints the token" >&2
    exit 2
    ;;
  esac
}

# The console's public test vector for a cluster key file: the seeds 11..42 and 1..32,
# never a real key. The same key is in crates/hippius-cli/tests/e2e/common.rs. A script
# that writes it adds both seeds to what it masks and scans for.
TEST_KEY_SIGN_SEED="CwwNDg8QERITFBUWFxgZGhscHR4fICEiIyQlJicoKSo="
TEST_KEY_BOX_SEED="AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA="

# write_test_cluster_key <file>: writes the test vector as a cluster key file. The caller
# sets umask 077 first, so the new file is 0600, as hipc requires.
write_test_cluster_key() {
  printf '{"version":1,"kind":"hippius-k8s-cluster-key",' >"$1"
  printf '"sign_secret_key":"%s","sign_public_key":"%s",' \
    "$TEST_KEY_SIGN_SEED" "VCgh6VKd0a0kO/3/XGQKZRlLSBaYemUN6GHXzp5BemM=" >>"$1"
  printf '"box_secret_key":"%s","box_public_key":"%s"}\n' \
    "$TEST_KEY_BOX_SEED" "B6N8vBQgk8i3VdwbEOhstCY3StFqqFPtC9/AsrhtHHw=" >>"$1"
}

# schema_route <hipc> <command path> <method> [value...]: each route that `hipc schema` lists
# for the command (`compute vms get`) with that method and one placeholder per value, one per
# line in the schema's order, with the values in its placeholders in order. A script reads a
# route from the binary under test this way rather than keep its own copy of the path, so a
# route that moves to another API version needs no edit here. Prints nothing when none fits,
# or when the binary cannot say, so a check that needs the route fails rather than the run.
schema_route() {
  schema_route_ending "$1" "$2" "$3" "" "${@:4}"
}

# schema_route_ending <hipc> <command path> <method> <ending> [value...]: as schema_route,
# only the routes whose template ends with <ending>, such as `/secrets/` or `{cluster_id}/`,
# for a command that sends to several routes with one method.
schema_route_ending() {
  local hipc="$1" command_words
  read -r -a command_words <<<"$2"
  "$hipc" schema "${command_words[@]}" </dev/null 2>/dev/null |
    jq -r --arg method "$3" --arg ending "$4" '
      $ARGS.positional as $values
      | .commands[0].routes[] | select(.method == $method) | .path
      | select(endswith($ending))
      | select([scan("\\{[^}]*\\}")] | length == ($values | length))
      | reduce $values[] as $value (.; sub("\\{[^}]*\\}"; $value))' --args "${@:5}" 2>/dev/null ||
    true
}
