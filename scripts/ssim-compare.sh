#!/usr/bin/env bash
# ssim-compare.sh <a.png> <b.png> <min-ssim>  — exits 1 if SSIM(All) < min
set -euo pipefail
command -v ffmpeg >/dev/null 2>&1 || {
  echo "ssim-compare: ffmpeg not found on PATH — install it (brew install ffmpeg / winget install ffmpeg)" >&2
  exit 2
}
# Capture ffmpeg's output separately from the score: a broken ffmpeg (bad
# install, missing codec, OOM) exits non-zero and prints no All:<score>,
# which would leave `score` empty. Empty coerces to 0 in the awk compare
# below and would be reported as a divergence — i.e. a comparison that
# never happened would look like a real visual failure. Exit 2 (same as a
# missing ffmpeg) so callers can tell a broken environment from a real
# divergence.
if ! output=$(ffmpeg -hide_banner -i "$1" -i "$2" -lavfi ssim -f null - 2>&1); then
  echo "ssim-compare: ffmpeg failed to compare the images:" >&2
  echo "$output" >&2
  exit 2
fi
score=$(printf '%s\n' "$output" | grep -oE "All:[0-9.]+" | cut -d: -f2)
if [ -z "$score" ]; then
  echo "ssim-compare: ffmpeg produced no SSIM score — cannot compare:" >&2
  echo "$output" >&2
  exit 2
fi
echo "SSIM=$score (min $3)"
awk -v s="$score" -v m="$3" 'BEGIN { exit (s+0 >= m+0) ? 0 : 1 }'
