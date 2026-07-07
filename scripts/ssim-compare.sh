#!/usr/bin/env bash
# ssim-compare.sh <a.png> <b.png> <min-ssim>  — exits 1 if SSIM(All) < min
set -euo pipefail
command -v ffmpeg >/dev/null 2>&1 || {
  echo "ssim-compare: ffmpeg not found on PATH — install it (brew install ffmpeg / winget install ffmpeg)" >&2
  exit 2
}
score=$(ffmpeg -hide_banner -i "$1" -i "$2" -lavfi ssim -f null - 2>&1 \
  | grep -oE "All:[0-9.]+" | cut -d: -f2)
echo "SSIM=$score (min $3)"
awk -v s="$score" -v m="$3" 'BEGIN { exit (s+0 >= m+0) ? 0 : 1 }'
