#!/usr/bin/env bash
# Prints the workspace version the PyPI wheel `hippius` is built at.
#
# Usage: scripts/pypi-version-check.sh [path to the workspace Cargo.toml]
#
# maturin takes the wheel's version from the workspace, so the release job compares this
# with the tag before it builds a wheel. It prints the version, or on failure prints why
# and exits 1: a manifest it cannot read, no version in [workspace.package], or a version
# that is not X.Y.Z.
set -euo pipefail

die() {
  echo "pypi-version-check: error: $*" >&2
  exit 1
}

if [[ $# -gt 1 ]]; then
  echo "usage: $0 [path to the workspace Cargo.toml]" >&2
  exit 2
fi
MANIFEST="${1:-$(dirname "$0")/../Cargo.toml}"
[[ -r "$MANIFEST" ]] || die "cannot read $MANIFEST"

# The version line of the [workspace.package] table, the one every crate inherits.
VERSION=$(awk '
  /^\[/ { in_table = ($0 == "[workspace.package]") }
  in_table && /^version *= *"/ { sub(/^version *= *"/, ""); sub(/".*$/, ""); print; exit }
' "$MANIFEST")
[[ -n "$VERSION" ]] || die "$MANIFEST has no version in [workspace.package]"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  die "the workspace version $VERSION is not X.Y.Z"
echo "$VERSION"
