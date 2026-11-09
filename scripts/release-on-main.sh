#!/usr/bin/env bash
# Refuses to release a commit that is not on main.
#
# Usage: GH_TOKEN=<token> scripts/release-on-main.sh <owner/repo> <commit sha>
#
# A tag can name any commit, one on a branch that was never reviewed or merged included, and
# the release workflow builds, signs and publishes whatever the tag names. This asks GitHub
# to compare main with the commit: `identical` (the commit is main's head) and `behind` (an
# ancestor of main) pass; `ahead`, `diverged`, an unknown commit, or a failed call exit 1.
# The release workflow runs it before anything is built, and again before the release is
# created.
set -euo pipefail

die() {
  echo "release-on-main: error: $*" >&2
  exit 1
}

if [[ $# -ne 2 ]]; then
  echo "usage: GH_TOKEN=<token> $0 <owner/repo> <commit sha>" >&2
  exit 2
fi
REPO="$1"
COMMIT="$2"
# The commit goes into an API path, so only a full lower-case hex SHA is taken.
[[ "$COMMIT" =~ ^[0-9a-f]{40}$ ]] || die "not a commit SHA: $COMMIT"
[[ "$REPO" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*$ ]] ||
  die "not an owner/repo: $REPO"

status=$(gh api "repos/${REPO}/compare/main...${COMMIT}" --jq .status) ||
  die "cannot compare ${COMMIT} with main in ${REPO}"
case "$status" in
identical | behind) echo "${COMMIT} is on main (${status})" ;;
*) die "${COMMIT} is not on main: compared with main it is ${status:-unknown}; tag a commit on main" ;;
esac
