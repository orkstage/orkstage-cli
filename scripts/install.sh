#!/bin/sh
# Installs the OrkStage CLI (`ork`) on Linux or macOS from a GitHub release.
#
#   curl -fsSL https://orkstage.io/install.sh | sh
#
# Environment:
#   ORK_VERSION        version to install, for example 0.2.0 (default: the latest release)
#   ORK_INSTALL_DIR    where to put the binary (default: $HOME/.local/bin)
#   ORK_DOWNLOAD_BASE  where the release files are (default: the GitHub release of ORK_VERSION);
#                      a mirror must serve the archive and SHA256SUMS side by side
#
# No root needed. The archive is checked against the release's SHA256SUMS before installing.
set -eu

REPO="orkstage/orkstage-cli"
INSTALL_DIR="${ORK_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*" >&2; }
fail() { say "error: $*"; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "$1 is required"; }

need curl
need tar
need uname

case "$(uname -s)" in
  Linux) os="unknown-linux-musl" ;;
  Darwin) os="apple-darwin" ;;
  *) fail "unsupported system $(uname -s); on Windows use install.ps1" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) fail "unsupported architecture $(uname -m)" ;;
esac
target="$arch-$os"

version="${ORK_VERSION:-}"
if [ -z "$version" ]; then
  # The latest release URL redirects to .../releases/tag/v<version>.
  url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") ||
    fail "cannot find the latest release of $REPO"
  version="${url##*/v}"
fi
version="${version#v}"
case "$version" in
  "" | */*) fail "cannot tell the version to install (set ORK_VERSION)" ;;
esac

base="${ORK_DOWNLOAD_BASE:-https://github.com/$REPO/releases/download/v$version}"
archive="ork-$version-$target.tar.gz"

tmp=$(mktemp -d 2>/dev/null || mktemp -d -t ork)
trap 'rm -rf -- "$tmp"' EXIT INT TERM

say "Downloading ork $version for $target"
curl -fsSL -o "$tmp/$archive" "$base/$archive" || fail "cannot download $base/$archive"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || fail "cannot download $base/SHA256SUMS"

expected=$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
[ -n "$expected" ] || fail "$archive is not listed in SHA256SUMS"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$tmp/$archive" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
  actual=$(shasum -a 256 "$tmp/$archive" | awk '{ print $1 }')
else
  fail "sha256sum or shasum is required to verify the download"
fi
[ "$expected" = "$actual" ] || fail "checksum mismatch for $archive (expected $expected, got $actual)"

tar -xzf "$tmp/$archive" -C "$tmp"
[ -f "$tmp/ork" ] || fail "the archive does not contain ork"
mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmp/ork" "$INSTALL_DIR/ork" 2>/dev/null || { cp "$tmp/ork" "$INSTALL_DIR/ork" && chmod 0755 "$INSTALL_DIR/ork"; }

say "Installed $("$INSTALL_DIR/ork" --version) to $INSTALL_DIR/ork"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) say "Add $INSTALL_DIR to your PATH, for example: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
