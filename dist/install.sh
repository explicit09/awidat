#!/usr/bin/env bash
# Awidat one-line installer.
#
# Usage (end-user):
#   curl -fsSL https://awidat.example/install.sh | sh
#
# What it does:
#   1. Detects the user's OS + arch (macOS arm64 / linux x86_64).
#   2. Downloads the matching release tarball.
#   3. Extracts to $AWIDAT_HOME (default: ~/.local/share/awidat).
#   4. Symlinks the awidat binary into ~/.local/bin/.
#   5. Runs `uv sync` once to materialize per-indexer venvs.
#
# Why this shape (vs PyOxidizer/Docker/shiv): the python workspace
# uses native extensions (torch, dlib, opencv) whose wheels are best
# resolved by uv against the user's machine. Embedding Python in the
# Rust binary fights that; Docker breaks MCP stdio for desktop users.
# Bundling uv + the python source tree is the boring/right path —
# uv handles the platform-specific wheel resolution upstream.
#
# This script is idempotent: re-running upgrades in place.

set -euo pipefail

AWIDAT_HOME="${AWIDAT_HOME:-$HOME/.local/share/awidat}"
BIN_DIR="${AWIDAT_BIN_DIR:-$HOME/.local/bin}"
RELEASE_BASE="${AWIDAT_RELEASE_BASE:-https://example.com/awidat/releases/latest}"

# Detect platform.
os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"
case "$os-$arch" in
  darwin-arm64)  triple="aarch64-apple-darwin" ;;
  darwin-x86_64) triple="x86_64-apple-darwin" ;;
  linux-x86_64)  triple="x86_64-unknown-linux-gnu" ;;
  linux-aarch64) triple="aarch64-unknown-linux-gnu" ;;
  *)
    echo "awidat: unsupported platform: $os-$arch" >&2
    exit 1
    ;;
esac

tarball_url="${RELEASE_BASE}/awidat-${triple}.tar.gz"
echo "Downloading awidat for ${triple}..."
mkdir -p "$AWIDAT_HOME"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

curl -fsSL "$tarball_url" -o "$tmpdir/awidat.tar.gz"
tar -xzf "$tmpdir/awidat.tar.gz" -C "$AWIDAT_HOME" --strip-components=1

# Symlink the binary into PATH. Don't clobber a non-symlink (someone
# wrote their own `awidat` script there); refuse and tell them.
mkdir -p "$BIN_DIR"
target="$BIN_DIR/awidat"
if [ -e "$target" ] && [ ! -L "$target" ]; then
  echo "awidat: refusing to overwrite non-symlink at $target." >&2
  echo "       Move it aside and re-run." >&2
  exit 1
fi
ln -snf "$AWIDAT_HOME/bin/awidat" "$target"

# Make sure uv is on PATH; we ship our own copy as a fallback.
if ! command -v uv >/dev/null 2>&1; then
  ln -snf "$AWIDAT_HOME/bin/uv" "$BIN_DIR/uv" 2>/dev/null || true
fi

# Materialize per-indexer venvs. First run downloads ~3 GB of wheels;
# subsequent installs hit uv's cache and are fast.
echo "Resolving python indexers (this is a one-time ~3 GB download)..."
( cd "$AWIDAT_HOME/python" && "$AWIDAT_HOME/bin/uv" sync --all-packages )

echo
echo "awidat installed."
echo "  Binary:  $target"
echo "  Data:    $AWIDAT_HOME"
echo
echo "Next steps:"
echo "  awidat new my-first-cut --import https://youtu.be/<id>"
echo
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    echo "Note: $BIN_DIR is not on your PATH. Add it:"
    echo "  echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.zshrc"
    ;;
esac
