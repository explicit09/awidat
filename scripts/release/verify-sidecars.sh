#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 1 ]]; then
  echo "usage: $0 TARGET_TRIPLE" >&2
  exit 2
fi

target="$1"
root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
binaries_dir="${RELEASE_BINARIES_DIR:-$root_dir/apps/desktop/src-tauri/binaries}"

case "$target" in
  aarch64-apple-darwin|x86_64-apple-darwin|x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu)
    suffix="-$target"
    ;;
  x86_64-pc-windows-msvc)
    suffix="-$target.exe"
    ;;
  *)
    echo "unsupported target triple: $target" >&2
    exit 2
    ;;
esac

required_sidecars=(
  codex
  ffmpeg
  ffprobe
  montage-mcp-server
  rg
  uv
  yt-dlp
)

check_sidecar() {
  local path="$1"
  if [[ ! -e "$path" ]]; then
    echo "missing sidecar: $path" >&2
    exit 1
  fi
  if [[ ! -x "$path" ]]; then
    echo "sidecar is not executable: $path" >&2
    exit 1
  fi
  if grep -Eaq "sidecar check stub|sidecar unavailable in CI compile check" "$path"; then
    echo "sidecar is a CI check stub, not a release binary: $path" >&2
    exit 1
  fi
}

for sidecar in "${required_sidecars[@]}"; do
  check_sidecar "$binaries_dir/$sidecar$suffix"
done

echo "release sidecars verified for $target"
