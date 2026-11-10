#!/usr/bin/env bash
# Checks that a release tag names the workspace version.
#
# Usage: scripts/release-version.sh <tag> [path to the workspace Cargo.toml]
#
# Every crate takes its version from [workspace.package], and maturin gives it to the PyPI
# wheel, so the release workflow runs this once, before anything is built. It prints the
# version when the tag is `v` followed by it. Otherwise it prints why and exits 1: a manifest
# it cannot read, no version in [workspace.package], a version that is not X.Y.Z, or a tag
# that names another version.
set -euo pipefail

die() {
  echo "release-version: error: $*" >&2
  exit 1
}

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 <tag> [path to the workspace Cargo.toml]" >&2
  exit 2
fi
TAG="$1"
MANIFEST="${2:-$(dirname "$0")/../Cargo.toml}"
[[ -r "$MANIFEST" ]] || die "cannot read $MANIFEST"

# The version line of the [workspace.package] table, the one every crate inherits.
VERSION=$(awk '
  /^\[/ { in_table = ($0 == "[workspace.package]") }
  in_table && /^version *= *"/ { sub(/^version *= *"/, ""); sub(/".*$/, ""); print; exit }
' "$MANIFEST")
[[ -n "$VERSION" ]] || die "$MANIFEST has no version in [workspace.package]"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  die "the workspace version $VERSION is not X.Y.Z"
[[ "$TAG" == "v$VERSION" ]] || die "tag $TAG does not name the workspace version $VERSION"
echo "$VERSION"
