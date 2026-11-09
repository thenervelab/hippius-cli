#!/usr/bin/env bash
# Packs a release binary into the tarball a release publishes, the same bytes from the same
# binary on any run: `hipc`, `LICENSE-MIT` and `LICENSE-APACHE` at the top of the archive,
# in that order, owned by 0:0 with modes 0755 and 0644, each dated SOURCE_DATE_EPOCH (the
# commit's time unless the caller sets it), and gzip storing no name and no time.
# scripts/install.sh extracts the member `hipc`, and the Homebrew formula installs `hipc`
# from the unpacked root.
#
# Usage: scripts/package.sh <binary> <tarball>
#
# GNU tar is required, as `gtar` (the macOS runner image has it) or as `tar`: bsdtar cannot
# set a member's time or owner this way. The members are named, so their order is the
# order given; --sort=name would order a directory's entries, of which there are none.
set -euo pipefail

die() {
  echo "package: $*" >&2
  exit 1
}

[[ $# == 2 ]] || {
  echo "usage: package.sh <binary> <tarball>" >&2
  exit 2
}
binary="$1"
tarball="$2"
root="$(cd "$(dirname "$0")/.." && pwd)"
[[ -f "$binary" ]] || die "no binary at $binary"

if [[ -z "${SOURCE_DATE_EPOCH:-}" ]]; then
  SOURCE_DATE_EPOCH="$(git -C "$root" log -1 --format=%ct)"
fi
[[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]] || die "SOURCE_DATE_EPOCH is not a number of seconds"

gnutar=""
for candidate in gtar tar; do
  if "$candidate" --version 2>/dev/null | grep -q 'GNU tar'; then
    gnutar="$candidate"
    break
  fi
done
[[ -n "$gnutar" ]] || die "GNU tar is required, as gtar or tar"

stage="$(mktemp -d "${TMPDIR:-/tmp}/hipc-package.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
cp "$binary" "$stage/hipc"
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$stage/"
chmod 0755 "$stage/hipc"
chmod 0644 "$stage/LICENSE-MIT" "$stage/LICENSE-APACHE"

"$gnutar" --create --format=gnu --mtime="@$SOURCE_DATE_EPOCH" \
  --owner=0 --group=0 --numeric-owner --file=- -C "$stage" \
  hipc LICENSE-MIT LICENSE-APACHE |
  gzip -9 -n >"$tarball"
