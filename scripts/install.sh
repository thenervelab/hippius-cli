#!/bin/sh
# Installs the hipc binary for this machine from the releases of thenervelab/hippius-cli. Each
# release carries this file as its asset install.sh, listed in the signed SHA256SUMS.
#
#   url=https://github.com/thenervelab/hippius-cli/releases/latest/download/install.sh
#   curl -q --proto '=https' --tlsv1.2 -fsSL "$url" | sh
#   curl -q --proto '=https' --tlsv1.2 -fsSL "$url" | sh -s -- 1.0.0
#   curl -q --proto '=https' --tlsv1.2 -fsSL "$url" | sh -s -- --version 1.0.0
#
# It downloads hipc-<target>.tar.gz and SHA256SUMS from the release, checks the tarball's
# hash, checks the signature of SHA256SUMS when cosign is on PATH, and installs hipc to
# HIPC_INSTALL_DIR, default ~/.local/bin. It never needs root and never edits a shell profile.
#
# HIPC_INSTALL_REQUIRE_SIGNATURE=1 makes a missing cosign an error instead of a note, for a
# pipeline that must not fall back to the hash check alone.
#
# HIPC_INSTALL_BASE_URL replaces https://github.com/<repo>/releases, but only when
# HIPC_INSTALL_ALLOW_BASE_URL=1 is set too, so a value inherited from the environment cannot
# point the downloads elsewhere on its own. The tests set both to reach a local server; they
# are not documented to users.
#
# Up to 0.14.0 the command was named hip, and the install directory HIP_INSTALL_DIR, the one
# variable that release documented. When only that old name is set, and the directory it
# names holds a hip, the 0.14.0 install it made, it is used, with a note. Any other
# HIP_INSTALL_DIR is ignored with a note: HIP_ is a common prefix, so a value set for another
# tool does not choose where hipc goes. The test-only base URL pair is not carried over. This
# script installs hipc releases only, 0.15.0 on.
#
# The script's own variables are lower case and each is set before it is read, so only the
# HIPC_INSTALL_ variables and HIP_INSTALL_DIR come from the environment.
#
# The whole body is in `main`, called on the last line, so a download cut short defines
# functions and runs nothing. The one exception, shared with the rustup and uv installers, is
# a cut right after the word `main` on that line: it runs `main` without the arguments, so a
# pinned version is ignored and the latest release is installed, fully verified.

# The repository whose releases hold the tarballs. Change it here and nowhere else in this
# file, except in hipc_signer below.
hipc_repo='thenervelab/hippius-cli'

# The workflow that signs SHA256SUMS, in the same repository. Its GitHub OIDC identity,
# followed by the tag being installed, is what cosign checks: a regular expression, so the
# dots are escaped. The bundle name must match the asset the release
# workflow uploads next to SHA256SUMS.
hipc_signer='^https://github\.com/thenervelab/hippius-cli/\.github/workflows/release\.yml@refs/tags/'
hipc_issuer='https://token.actions.githubusercontent.com'
hipc_bundle='SHA256SUMS.sigstore.json'

say() {
  printf 'hipc-install: %s\n' "$1" >&2
}

die() {
  say "error: $1"
  exit 1
}

usage() {
  cat >&2 <<'EOF'
usage: install.sh [VERSION]
       install.sh --version VERSION

Installs hipc to $HIPC_INSTALL_DIR (default ~/.local/bin). VERSION is X.Y.Z or vX.Y.Z; the
default is the latest release. HIPC_INSTALL_REQUIRE_SIGNATURE=1 fails the install when cosign
is not on PATH to check the signature of SHA256SUMS.

Uninstall: remove $HIPC_INSTALL_DIR/hipc (default ~/.local/bin/hipc). The config stays in
~/.hippius; remove it too to forget the token.
EOF
}

# Sets old_dir to HIP_INSTALL_DIR when only the old name is set and the directory holds a
# hip (a file or a symlink, dangling or not), and notes the old name whenever it is set.
adopt_old_dir() {
  old_dir=
  [ -n "${HIP_INSTALL_DIR:-}" ] || return 0
  if [ -n "${HIPC_INSTALL_DIR:-}" ]; then
    say "note: HIP_INSTALL_DIR is now HIPC_INSTALL_DIR; using HIPC_INSTALL_DIR"
  elif [ -e "$HIP_INSTALL_DIR/hip" ] || [ -L "$HIP_INSTALL_DIR/hip" ]; then
    say "note: HIP_INSTALL_DIR is now HIPC_INSTALL_DIR; using HIP_INSTALL_DIR"
    old_dir=$HIP_INSTALL_DIR
  else
    say "note: ignoring HIP_INSTALL_DIR: it holds no hip from an earlier install; \
set HIPC_INSTALL_DIR to choose where hipc goes"
  fi
}

# Prints the release target of this machine, or exits naming what is not supported.
detect_target() {
  os=$(uname -s) || die 'uname -s failed'
  arch=$(uname -m) || die 'uname -m failed'
  case "$arch" in
  arm64 | aarch64) arch=aarch64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) die "no hipc build for the $arch architecture; builds exist for arm64 and x86_64" ;;
  esac
  case "$os" in
  Darwin)
    # A shell under Rosetta reports x86_64 on Apple silicon. The arm64 build is the one
    # that matters there (the Intel build is a courtesy), so prefer it.
    if [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
      arch=aarch64
    fi
    echo "$arch-apple-darwin"
    ;;
  # The static musl build runs on any distribution, glibc or not.
  Linux) echo "$arch-unknown-linux-musl" ;;
  # Git Bash, MSYS2, and Cygwin report these; the Linux build runs under WSL.
  MINGW* | MSYS* | CYGWIN*)
    die "no hipc build for Windows ($os); run this under WSL, where the Linux build installs"
    ;;
  *) die "no hipc build for $os; builds exist for macOS and Linux" ;;
  esac
}

# Prints the release tag for the version argument, or for the latest release.
release_tag() {
  if [ -n "$1" ]; then
    version=${1#v}
  else
    version=$(latest_tag) || exit 1
    version=${version#v}
  fi
  case "$version" in
  '' | *[!0-9.]* | .* | *. | *..*)
    die "not a version: ${1:-$version} (expected X.Y.Z, such as 1.0.0)"
    ;;
  esac
  echo "v$version"
}

# Prints the latest release's tag, the last path segment of where releases/latest redirects
# to. The redirect is not rate limited the way the GitHub API is for an anonymous caller.
# HEAD requests: only the final URL matters, not the release page behind it.
latest_tag() {
  if [ "$hipc_downloader" = curl ]; then
    url=$(curl_run -I -o /dev/null -w '%{url_effective}' "$hipc_base/latest") ||
      die "cannot reach $hipc_base/latest"
  else
    # A failure here (a 404) is read below as no redirect at all, after wget's reason.
    wget_run -S --spider -O /dev/null "$hipc_base/latest" 2>"$hipc_tmp/latest.headers" ||
      wget_reason "$hipc_tmp/latest.headers"
    refuse_downgrade "$hipc_tmp/latest.headers"
    # wget prints each response's headers indented, then its own "Location: <url>
    # [following]" line; the first word after "Location:" is the URL either way.
    url=$(tr -d '\r' <"$hipc_tmp/latest.headers" |
      sed -n 's/^ *Location: *\([^ ]*\).*$/\1/p' | tail -n 1)
  fi
  [ -n "$url" ] || die "cannot find the latest release at $hipc_base/latest"
  echo "${url##*/}"
}

# curl with only the protocols hipc_proto allows, also on a redirect, and TLS 1.2 or later.
# -q, which must come first, skips ~/.curlrc, where `insecure` would turn off certificate
# checks. A transient failure (a timeout, a 5xx) is retried three times, and a connection
# that never answers gives up after 15 seconds rather than hanging the pipe. A download
# gives up after 15 minutes in all, or once it has run slower than 1 KiB/s for 30 seconds,
# and refuses a file over 200 MiB, which no release tarball comes near, so a broken or
# hostile server cannot hold the pipe open or fill TMPDIR.
curl_run() {
  curl -q --proto "$hipc_proto" --tlsv1.2 -fsSL --retry 3 --retry-delay 1 \
    --connect-timeout 15 --max-time 900 --speed-limit 1024 --speed-time 30 \
    --max-filesize 209715200 "$@"
}

# wget without ~/.wgetrc or /etc/wgetrc, where `check_certificate = off` would turn off
# certificate checks, with the retries and timeout of curl_run. wget has no total-time,
# speed, or size limit for one file: its --timeout bounds each connect and read, so a server
# that trickles bytes can keep it going; curl, tried first, has all three.
wget_run() {
  if [ "$hipc_proto" = '=https' ]; then
    wget --no-config --https-only --tries=3 --timeout=15 "$@"
  else
    wget --no-config --tries=3 --timeout=15 "$@"
  fi
}

# Exits when the wget response headers in file $1 redirect an HTTPS download to plain HTTP.
# wget has no switch that refuses that (--https-only governs only the links of a recursive
# download), so the redirects are read back from the headers.
refuse_downgrade() {
  [ "$hipc_proto" = '=https' ] || return 0
  if grep -i -q '^ *Location: *http:' "$1"; then
    die "refused a redirect from HTTPS to plain HTTP; nothing was installed"
  fi
}

# Prints the last lines wget itself wrote to the headers file $1: its messages are the
# unindented, non-blank lines, the headers the indented ones.
wget_reason() {
  grep -v -e '^ ' -e '^$' "$1" | tail -n 3 >&2
}

# Downloads URL $1 to file $2. curl prints its own reason for a failure; wget's goes to the
# headers file with the headers, so it is shown from there instead. On a failure,
# fetch_missing is 1 when the server answered 404, the one status that says the file is not
# in the release, and empty otherwise.
fetch() {
  fetch_missing=
  if [ "$hipc_downloader" = curl ]; then
    # -w prints the last response's status, a failed one included; 000 when none came.
    fetch_status=$(curl_run -o "$2" -w '%{http_code}' "$1") || {
      [ "$fetch_status" != 404 ] || fetch_missing=1
      return 1
    }
  else
    wget_run -S -O "$2" "$1" 2>"$2.headers" || {
      wget_reason "$2.headers"
      # The last status line wget printed, `  HTTP/1.1 404 Not Found`.
      fetch_status=$(awk '$1 ~ /^HTTP\// { code = $2 } END { print code }' "$2.headers")
      [ "$fetch_status" != 404 ] || fetch_missing=1
      return 1
    }
    refuse_downgrade "$2.headers"
  fi
}

# Exits naming why the download of URL $1 failed: a 404 is what $2 says is missing, anything
# else the reason curl or wget printed above.
fetch_failed() {
  if [ -n "$fetch_missing" ]; then
    die "cannot download $1 ($2)"
  fi
  die "cannot download $1; see the reason above"
}

# Prints path $1 without doubled or trailing slashes (a HOME written with one gives
# HOME//.local/bin), so two spellings of one directory compare equal and the messages read
# cleanly. The root directory keeps its one slash.
clean_path() {
  cp_path=$1
  while [ "$cp_path" != "${cp_path%//*}" ]; do
    cp_path="${cp_path%%//*}/${cp_path#*//}"
  done
  while [ "$cp_path" != / ] && [ "${cp_path%/}" != "$cp_path" ]; do
    cp_path=${cp_path%/}
  done
  echo "$cp_path"
}

# Prints the SHA-256 of file $1, in lower-case hex.
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sum=$(sha256sum "$1") || die "sha256sum $1 failed"
  else
    sum=$(shasum -a 256 "$1") || die "shasum $1 failed"
  fi
  echo "${sum%% *}"
}

# Checks file $1 against its line in SHA256SUMS file $2. A missing line is a failure.
verify_hash() {
  asset=${1##*/}
  # sha256sum writes "<hash>  <name>", or "<hash> *<name>" in binary mode.
  expected=$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1 }' "$2")
  case "$expected" in
  '' | *[!0-9a-f]*) die "SHA256SUMS has no single well-formed line for $asset" ;;
  esac
  [ ${#expected} -eq 64 ] || die "SHA256SUMS has no single well-formed line for $asset"
  actual=$(sha256_of "$1")
  [ "$actual" = "$expected" ] ||
    die "$asset does not match SHA256SUMS (expected $expected, got $actual); nothing was installed"
}

# Checks the signature of SHA256SUMS ($1) of release tag $2 when cosign is on PATH. The
# certificate must name the release workflow of thenervelab/hippius-cli, run for exactly
# that tag, so the SHA256SUMS of an older release served under a newer tag is refused.
verify_signature() {
  url="$hipc_base/download/$2/$hipc_bundle"
  if ! command -v cosign >/dev/null 2>&1; then
    if [ "${HIPC_INSTALL_REQUIRE_SIGNATURE:-}" = 1 ]; then
      die 'cosign is not on PATH and HIPC_INSTALL_REQUIRE_SIGNATURE=1 is set; nothing was installed'
    fi
    say "note: signature not checked: cosign is not on PATH; install cosign and rerun to check"
    return
  fi
  # The release signs with cosign 3, whose bundle cosign 1.x and 2.x cannot read (2.4 only
  # with a switch), so their failure would read as a bad signature. The version banner has
  # a GitVersion line; an unreadable banner is left for verify-blob to judge.
  cosign_version=$(cosign version 2>&1) || :
  cosign_major=$(printf '%s\n' "$cosign_version" |
    awk '$1 == "GitVersion:" { sub(/^v/, "", $2); sub(/\..*$/, "", $2); print $2; exit }')
  case "$cosign_major" in
  '' | *[!0-9]*) cosign_major= ;;
  esac
  if [ -n "$cosign_major" ] && [ "$cosign_major" -lt 3 ]; then
    die "cosign 3.0 or later is needed to check the Sigstore bundle; \
this is cosign $cosign_major.x; nothing was installed"
  fi
  fetch "$url" "$1.bundle" || fetch_failed "$url" "release $2 has no signature bundle"
  # The tag holds only v, digits and dots (release_tag checked it).
  identity="$hipc_signer$(printf '%s' "$2" | sed 's/\./\\./g')\$"
  if ! out=$(cosign verify-blob --bundle "$1.bundle" \
    --certificate-identity-regexp "$identity" \
    --certificate-oidc-issuer "$hipc_issuer" "$1" 2>&1); then
    printf '%s\n' "$out" >&2
    die "the signature of SHA256SUMS does not verify; nothing was installed"
  fi
  say "SHA256SUMS signature verified."
}

# Moves the binary $1 into directory $2 as hipc, and sets hipc_installed to what it prints
# for --version. The copy goes to a temporary name in the same directory and runs there
# first, so a binary that does not run on this machine never replaces a working hipc, and
# the rename is atomic: a running hipc is never half-written.
install_binary() {
  # A directory this creates is readable by everyone, whatever the caller's umask.
  (umask 022 && mkdir -p "$2") || die "cannot create $2"
  # Checked before the downloads too; again here, in case one appeared since. mv would
  # move the copy into a directory of that name and report success.
  [ ! -d "$2/hipc" ] ||
    die "$2/hipc is a directory; remove or rename it first; nothing was installed"
  hipc_partial=$(mktemp "$2/.hipc.XXXXXX") || die "cannot write to $2"
  cp "$1" "$hipc_partial" || die "cannot write to $2"
  chmod 755 "$hipc_partial" || die "cannot make $hipc_partial executable"
  hipc_installed=$("$hipc_partial" --version </dev/null) ||
    die "the downloaded hipc does not run on this machine (or $2 is mounted noexec); \
nothing was installed"
  mv -f "$hipc_partial" "$2/hipc" || die "cannot replace $2/hipc"
  hipc_partial=
}

cleanup() {
  # rm, not trash: this runs on the user's machine, where trash is not installed.
  [ -z "$hipc_tmp" ] || rm -rf "$hipc_tmp"
  [ -z "$hipc_partial" ] || rm -f "$hipc_partial"
}

# Sets hipc_downloader to curl, or to wget when only a wget with the switches wget_run needs
# is on PATH.
pick_downloader() {
  if command -v curl >/dev/null 2>&1; then
    hipc_downloader=curl
    return
  fi
  command -v wget >/dev/null 2>&1 || die 'neither curl nor wget is on PATH'
  hipc_downloader=wget
  # BusyBox wget has neither switch.
  wget_help=$(wget --help 2>&1) || :
  case "$wget_help" in
  *--no-config*) ;;
  *) die 'this wget has no --no-config (BusyBox?); install curl or GNU wget' ;;
  esac
  case "$wget_help" in
  *--https-only*) ;;
  *) die 'this wget has no --https-only (BusyBox?); install curl or GNU wget' ;;
  esac
}

main() {
  set -eu
  requested=
  while [ $# -gt 0 ]; do
    case "$1" in
    -h | --help)
      usage
      exit 0
      ;;
    --version)
      [ $# -ge 2 ] || die '--version needs a value, such as --version 1.0.0'
      requested=$2
      shift
      ;;
    --version=*) requested=${1#--version=} ;;
    -*)
      usage
      die "unknown option: $1"
      ;;
    *) requested=$1 ;;
    esac
    shift
  done

  adopt_old_dir
  if [ -n "${HIPC_INSTALL_DIR:-}" ]; then
    dir=$HIPC_INSTALL_DIR
    dir_from=HIPC_INSTALL_DIR
  elif [ -n "$old_dir" ]; then
    dir=$old_dir
    dir_from=HIP_INSTALL_DIR
  elif [ -n "${HOME:-}" ]; then
    dir=$HOME/.local/bin
    dir_from=HOME
  else
    die 'HOME is not set; set HIPC_INSTALL_DIR to the directory to install hipc in'
  fi
  dir=$(clean_path "$dir")
  # A relative directory, or a `~` the shell did not expand (quoted), would install
  # somewhere that only works from this directory, with a PATH line to match.
  case "$dir" in
  /*) ;;
  '~'*) die "$dir_from must not start with a literal ~; use \$HOME or an absolute path" ;;
  *) die "$dir_from must be an absolute path, not ${dir%/.local/bin}" ;;
  esac
  # Before any download; install_binary checks again, in case one appeared since.
  [ ! -d "$dir/hipc" ] ||
    die "$dir/hipc is a directory; remove or rename it first; nothing was installed"

  pick_downloader
  if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
    die 'neither sha256sum nor shasum is on PATH, so the download cannot be checked'
  fi

  hipc_base=https://github.com/$hipc_repo/releases
  if [ -n "${HIPC_INSTALL_BASE_URL:-}" ]; then
    if [ "${HIPC_INSTALL_ALLOW_BASE_URL:-}" = 1 ]; then
      hipc_base=$HIPC_INSTALL_BASE_URL
    else
      say "note: ignoring HIPC_INSTALL_BASE_URL; it needs HIPC_INSTALL_ALLOW_BASE_URL=1 too"
    fi
  fi
  # Only an explicit http:// base (the tests' local server) allows plain HTTP.
  case "$hipc_base" in
  http://*) hipc_proto='=http,https' ;;
  *) hipc_proto='=https' ;;
  esac

  target=$(detect_target)

  hipc_tmp=
  hipc_partial=
  trap cleanup EXIT
  # 128 + the signal, as a shell killed by it would exit, so a caller can tell an
  # interruption from a failure; `exit` runs the EXIT trap, so the temporary files go.
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM
  hipc_tmp=$(mktemp -d) || die 'cannot create a temporary directory'
  tag=$(release_tag "$requested")

  asset="hipc-$target.tar.gz"
  say "installing hipc $tag for $target to $dir from $hipc_base"
  fetch "$hipc_base/download/$tag/$asset" "$hipc_tmp/$asset" ||
    fetch_failed "$hipc_base/download/$tag/$asset" \
      "no release $tag, or no build for $target in it"
  fetch "$hipc_base/download/$tag/SHA256SUMS" "$hipc_tmp/SHA256SUMS" ||
    fetch_failed "$hipc_base/download/$tag/SHA256SUMS" "release $tag has no SHA256SUMS"
  verify_signature "$hipc_tmp/SHA256SUMS" "$tag"
  verify_hash "$hipc_tmp/$asset" "$hipc_tmp/SHA256SUMS"

  mkdir "$hipc_tmp/x"
  tar -xzf "$hipc_tmp/$asset" -C "$hipc_tmp/x" hipc || die "cannot unpack $asset"
  # Only a regular file is installed. A symlink member, which tar keeps as a link, would
  # copy and run whatever file on this machine it points at; a duplicate member is
  # extracted last, so this checks the one that would be installed.
  if [ -L "$hipc_tmp/x/hipc" ] || [ ! -f "$hipc_tmp/x/hipc" ]; then
    die "the hipc in $asset is not a regular file; nothing was installed"
  fi
  install_binary "$hipc_tmp/x/hipc" "$dir"

  say "installed $hipc_installed to $dir/hipc"
  # Each PATH entry is cleaned as dir was, so one written with a trailing or doubled slash
  # still counts. IFS is only the colon, so an entry with a space stays whole.
  on_path=
  ifs_save=$IFS
  IFS=:
  for entry in ${PATH:-}; do
    [ "$(clean_path "$entry")" != "$dir" ] || on_path=1
  done
  IFS=$ifs_save
  [ -n "$on_path" ] ||
    say "$dir is not on PATH; add this line to your shell profile: export PATH=\"$dir:\$PATH\""
  # Another hipc earlier on PATH, such as one from `cargo install`, keeps running instead.
  # The shell spells the one found after the PATH entry, slashes and all, so it is cleaned
  # as dir was before the two are compared.
  found=$(command -v hipc 2>/dev/null) || :
  if [ -n "$found" ] && [ "$(clean_path "$found")" != "$dir/hipc" ]; then
    say "note: $found comes first on PATH and runs instead of $dir/hipc"
  fi
  # hip was this command's name up to 0.14.0, and an upgrade leaves that binary behind.
  # Another program may share the name, so the note asks rather than tells. A dangling
  # symlink, left when its target was removed, counts too: it is the same leftover.
  old_note='may be this tool under its old name (hip, up to 0.14.0); if so, remove it'
  if [ -e "$dir/hip" ] || [ -L "$dir/hip" ]; then
    say "note: $dir/hip $old_note"
  fi
  found=$(command -v hip 2>/dev/null) || :
  if [ -n "$found" ] && [ "$(clean_path "$found")" != "$dir/hip" ]; then
    say "note: $found $old_note"
  fi
}

main "$@"
