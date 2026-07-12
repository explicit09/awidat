#!/usr/bin/env bash
# Cumulative Phase-1 verification gate. Later tasks append lines; the loop
# runs this before every commit that claims a task done.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
scripts/loop-cargo.sh test -p montage-render --test animation_vectors
( cd apps/desktop && npm test )
