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

# Upgrade-safe install layout:
#
#   ~/.local/share/awidat/               <- AWIDAT_HOME (symlink target dir)
#     current -> versions/<sha>/         <- atomic swap point
#     versions/<sha>/                    <- per-version content
#       bin/awidat
#       bin/uv
#       python/...
#
# Why the indirection: `awidat upgrade` runs while the *current*
# binary is still executing. On macOS, overwriting an executable's
# text pages while it's running gets the kernel's `Killed: 9`. By
# extracting the new version to a sibling dir and swapping the
# `current` symlink atomically, the running process keeps its
# inode, the new process picks up the new binary on next launch.
#
# Sha is the SHA256 of the tarball, truncated to 12 chars. Using
# the content hash means re-installing the same tarball is a no-op
# (the dir already exists) and concurrent installs don't race.
sha="$(shasum -a 256 "$tmpdir/awidat.tar.gz" | awk '{print substr($1, 1, 12)}')"
versions_dir="$AWIDAT_HOME/versions"
new_version_dir="$versions_dir/$sha"
mkdir -p "$versions_dir"

# Migration: if a previous install used the flat layout (extracted
# directly into AWIDAT_HOME, no `current/` symlink, no `versions/`
# subtree), tidy up the orphaned bin/ + python/ + share/ trees
# before laying down the new versioned install. We don't preserve
# the old install — it gets replaced — but we DO move it aside
# first so a Ctrl-C mid-install leaves something the user can
# recover from.
if [ ! -L "$AWIDAT_HOME/current" ] && [ -d "$AWIDAT_HOME/bin" ]; then
  echo "Migrating from flat install layout to versioned..."
  graveyard="$AWIDAT_HOME/.pre-versioned-$(date +%s)"
  mkdir -p "$graveyard"
  for dir in bin python share VERSION; do
    [ -e "$AWIDAT_HOME/$dir" ] && mv "$AWIDAT_HOME/$dir" "$graveyard/" 2>/dev/null || true
  done
  echo "  Old layout preserved at $graveyard — safe to delete after"
  echo "  verifying the new install works."
fi

if [ -d "$new_version_dir" ]; then
  echo "Version $sha already present; skipping extraction."
else
  staging="$(mktemp -d -p "$versions_dir" .staging.XXXXXX)"
  tar -xzf "$tmpdir/awidat.tar.gz" -C "$staging" --strip-components=1
  mv "$staging" "$new_version_dir"
fi

# Atomic-swap the `current` symlink. If a previous install used the
# old layout (extracted directly into $AWIDAT_HOME), we leave that
# alone — the symlink takes precedence on PATH lookups.
ln -snf "$new_version_dir" "$AWIDAT_HOME/current"

# GC: keep the 3 most-recently-modified versions, delete the rest.
# Conservative — old versions are tiny (~24 MB each) and rolling
# back to the previous one has historically saved bacon. Skip GC
# when AWIDAT_HOME points anywhere weird.
if [ -d "$versions_dir" ]; then
  ls -1t "$versions_dir" 2>/dev/null \
    | tail -n +4 \
    | while read -r old; do
        [ -n "$old" ] && rm -rf "$versions_dir/$old"
      done
fi

# Symlink the binary into PATH. Don't clobber a non-symlink (someone
# wrote their own `awidat` script there); refuse and tell them.
mkdir -p "$BIN_DIR"
target="$BIN_DIR/awidat"
if [ -e "$target" ] && [ ! -L "$target" ]; then
  echo "awidat: refusing to overwrite non-symlink at $target." >&2
  echo "       Move it aside and re-run." >&2
  exit 1
fi
ln -snf "$AWIDAT_HOME/current/bin/awidat" "$target"

# Make sure uv is on PATH; we ship our own copy as a fallback.
if ! command -v uv >/dev/null 2>&1; then
  ln -snf "$AWIDAT_HOME/current/bin/uv" "$BIN_DIR/uv" 2>/dev/null || true
fi

# Materialize per-indexer venvs. First run downloads ~3 GB of wheels;
# subsequent installs hit uv's cache and are fast.
#
# AWIDAT_SKIP_UV_SYNC=1 skips this step. Useful for:
#   - Local-dev installs when the user already has a working .venv
#     elsewhere (uv's wheel cache is shared across venvs anyway, but
#     materializing another tree doubles disk usage).
#   - CI smoke-tests of the install flow that don't need real wheels.
# Without uv sync, the indexers can't run — but `awidat init`,
# `awidat new`, `awidat tui` (the Rust-side surface) work, and the
# user can run uv sync themselves later.
python_dir="$AWIDAT_HOME/current/python"
uv_bin="$AWIDAT_HOME/current/bin/uv"
if [ -n "${AWIDAT_SKIP_UV_SYNC:-}" ]; then
  echo "Skipping uv sync (AWIDAT_SKIP_UV_SYNC set)."
  echo "  To finish setup later, run:"
  echo "    cd $python_dir && uv sync --all-packages"
else
  echo "Resolving python indexers (this is a one-time ~3 GB download)..."
  ( cd "$python_dir" && "$uv_bin" sync --all-packages )
fi

echo
echo "awidat installed."
echo "  Binary:  $target"
echo "  Version: $sha"
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
