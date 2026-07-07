#!/usr/bin/env bash
# Cumulative Phase-1 verification gate. Later tasks append lines; the loop
# runs this before every commit that claims a task done.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
( cd apps/desktop && npm test )
( cd apps/desktop && npm run test:stage-harness )
