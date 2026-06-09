#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 1 ]]; then
  echo "usage: $0 ARTIFACT_DIR" >&2
  exit 2
fi

artifact_dir="$1"
if [[ ! -d "$artifact_dir" ]]; then
  echo "artifact directory does not exist: $artifact_dir" >&2
  exit 1
fi

found=0
while IFS= read -r -d '' artifact; do
  found=1
  (
    cd "$artifact_dir"
    shasum -a 256 "$(basename "$artifact")" > "$(basename "$artifact").sha256"
  )
done < <(find "$artifact_dir" -maxdepth 1 -type f \( -name '*.dmg' -o -name '*.zip' -o -name '*.tar.gz' -o -name '*.msi' -o -name '*.exe' \) -print0 | sort -z)

if [[ "$found" -eq 0 ]]; then
  echo "no release artifacts found in $artifact_dir" >&2
  exit 1
fi
