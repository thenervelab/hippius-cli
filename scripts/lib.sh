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
