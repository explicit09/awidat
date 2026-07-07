#!/usr/bin/env bash
# Cargo wrapper for the autonomous loop. The internal disk cannot hold build
# artifacts; the external drive can, but drops under sustained write load —
# so: mount-guard before and after, and never run unscoped workspace builds.
set -euo pipefail
MOUNT_POINT="/Volumes/My Passport for Mac"
if [ ! -d "$MOUNT_POINT" ]; then
  echo "loop-cargo: Passport drive not mounted — halting (do NOT build on internal disk)" >&2
  exit 86
fi
EXT_TARGET="$MOUNT_POINT/awidat-build/target"
mkdir -p "$EXT_TARGET"
status=0
CARGO_TARGET_DIR="$EXT_TARGET" cargo "$@" || status=$?
if [ ! -d "$MOUNT_POINT" ]; then
  echo "loop-cargo: drive dropped during build — artifacts suspect, retry once from scratch" >&2
  exit 87
fi
exit $status
