#!/usr/bin/env bash
# Cargo wrapper for the autonomous loop. The internal disk cannot hold build
# artifacts; the external drive can, but drops under sustained write load —
# so: mount-guard before and after, and never run unscoped workspace builds.
set -euo pipefail
EXT_TARGET="/Volumes/My Passport for Mac/awidat-build/target"
if [ ! -d "$(dirname "$EXT_TARGET")" ]; then
  echo "loop-cargo: Passport drive not mounted — halting (do NOT build on internal disk)" >&2
  exit 86
fi
CARGO_TARGET_DIR="$EXT_TARGET" cargo "$@"
status=$?
if [ ! -d "$(dirname "$EXT_TARGET")" ]; then
  echo "loop-cargo: drive dropped during build — artifacts suspect, retry once from scratch" >&2
  exit 87
fi
exit $status
