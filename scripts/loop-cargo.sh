#!/usr/bin/env bash
# Cargo wrapper for the autonomous loop. The internal disk cannot hold build
# artifacts; the external drive can, but drops under sustained write load —
# so: mount-guard before and after, and never run unscoped workspace builds.
set -euo pipefail
MOUNT_POINT="/Volumes/My Passport for Mac"
# Marker file distinguishes the real drive from a stale /Volumes directory
# left by an unclean unmount (this machine is prone to those) — a bare -d
# check would let mkdir recreate the tree on the internal disk.
MARKER="$MOUNT_POINT/awidat-build/.on-passport"
if [ ! -e "$MARKER" ]; then
  echo "loop-cargo: Passport drive not mounted (marker $MARKER missing) — halting (do NOT build on internal disk)" >&2
  exit 86
fi
EXT_TARGET="$MOUNT_POINT/awidat-build/target"
mkdir -p "$EXT_TARGET"
status=0
CARGO_TARGET_DIR="$EXT_TARGET" cargo "$@" || status=$?
if [ ! -e "$MARKER" ]; then
  echo "loop-cargo: drive dropped during build — artifacts suspect, retry once from scratch" >&2
  exit 87
fi
exit $status
