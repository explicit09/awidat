#!/usr/bin/env bash
# Build a release tarball for the current platform.
#
# Output: dist/build/awidat-<triple>.tar.gz containing:
#   bin/awidat                    — the Rust CLI
#   bin/uv                        — vendored uv (so installs don't
#                                   require the user to have uv)
#   python/                       — uv workspace tree (sources +
#                                   pyproject.toml + uv.lock; no
#                                   .venv — that gets built by the
#                                   installer's `uv sync` step)
#   share/awidat/install.sh       — copy of the installer for
#                                   self-installing tarballs
#   VERSION                       — `awidat --version` snapshot
#
# Why no .venv: torch + dlib wheels need to match the user's
# Python ABI / glibc / libstdc++; pre-baking them on a CI runner
# may not work on a different distro. uv's resolver runs on the
# end machine — that's the engineering tradeoff documented in
# install.sh.
#
# Usage:
#   ./dist/package.sh                 # builds for the current host
#   AWIDAT_PROFILE=release ./dist/package.sh   # release profile
#   AWIDAT_TARBALL=/path ./dist/package.sh     # custom output path

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${AWIDAT_PROFILE:-release}"
BUILD_DIR="${AWIDAT_BUILD_DIR:-$ROOT/dist/build}"

# Triple detection. We'd ideally read from `rustc -vV` but that's
# overkill for the supported set; uname covers it.
os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"
case "$os-$arch" in
  darwin-arm64)   triple="aarch64-apple-darwin" ;;
  darwin-x86_64)  triple="x86_64-apple-darwin" ;;
  linux-x86_64)   triple="x86_64-unknown-linux-gnu" ;;
  linux-aarch64)  triple="aarch64-unknown-linux-gnu" ;;
  *)
    echo "package.sh: unsupported platform: $os-$arch" >&2
    exit 1
    ;;
esac

stage="$BUILD_DIR/awidat-$triple"
rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/share/awidat"

# 1. Build the Rust binary.
echo "[package] cargo build --profile=$PROFILE -p awidat-cli"
( cd "$ROOT" && cargo build --profile "$PROFILE" -p awidat-cli )

# `release` profile lives at target/release; named profiles (e.g.
# 'dist') would be at target/<name>. We only ship release.
case "$PROFILE" in
  release) bin_src="$ROOT/target/release/awidat" ;;
  dev|debug) bin_src="$ROOT/target/debug/awidat" ;;
  *) bin_src="$ROOT/target/$PROFILE/awidat" ;;
esac
cp "$bin_src" "$stage/bin/awidat"
strip "$stage/bin/awidat" 2>/dev/null || true

# 2. Vendor uv. Required so the installer doesn't depend on the
# user already having uv. We just take whichever uv is on the
# build machine — it'll be the right triple by construction since
# we're packaging for the host.
if command -v uv >/dev/null 2>&1; then
  cp "$(command -v uv)" "$stage/bin/uv"
else
  echo "[package] uv not found on PATH; skipping vendored uv (installer will require uv)" >&2
fi

# 3. Copy the python workspace. Sources + lockfile + pyproject;
# explicitly NOT the .venv (per the why-no-venv comment above).
echo "[package] copying python workspace"
mkdir -p "$stage/python"
# Use rsync to filter; falls back to cp on systems without rsync.
if command -v rsync >/dev/null 2>&1; then
  rsync -a \
    --exclude '.venv/' \
    --exclude '__pycache__/' \
    --exclude '*.pyc' \
    --exclude '.pytest_cache/' \
    "$ROOT/python/" "$stage/python/"
else
  cp -R "$ROOT/python/." "$stage/python/"
  find "$stage/python" -type d -name '.venv' -prune -exec rm -rf {} +
  find "$stage/python" -type d -name '__pycache__' -prune -exec rm -rf {} +
fi

# 4. Bundled installer copy + version stamp.
cp "$ROOT/dist/install.sh" "$stage/share/awidat/install.sh"
# VERSION file: one line each for cargo version, build timestamp,
# git commit (when available). Single-purpose, machine-friendly —
# `awidat upgrade --check` parses just the first line.
{
  "$stage/bin/awidat" version 2>/dev/null | head -n1 || echo "awidat unknown"
  echo "built: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  if git -C "$ROOT" rev-parse --short HEAD >/dev/null 2>&1; then
    echo "commit: $(git -C "$ROOT" rev-parse --short HEAD)"
  fi
} > "$stage/VERSION"

# 5. Tar it up.
out="${AWIDAT_TARBALL:-$BUILD_DIR/awidat-$triple.tar.gz}"
echo "[package] tarring -> $out"
( cd "$BUILD_DIR" && tar -czf "$out" "awidat-$triple" )

# 6. Sanity print.
size_kb="$(du -k "$out" | awk '{print $1}')"
echo
echo "Built awidat-$triple.tar.gz (${size_kb} KB)"
echo "  $out"
echo
echo "To install on this machine for testing:"
echo "  AWIDAT_RELEASE_BASE=file://$BUILD_DIR \\"
echo "    bash $stage/share/awidat/install.sh"
