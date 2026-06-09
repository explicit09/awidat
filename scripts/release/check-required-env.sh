#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -eq 0 ]]; then
  echo "usage: $0 VAR_NAME [ADDITIONAL_VAR_NAME]" >&2
  exit 2
fi

for name in "$@"; do
  if [[ -z "${!name:-}" ]]; then
    echo "missing required environment variable: $name" >&2
    exit 1
  fi
done
