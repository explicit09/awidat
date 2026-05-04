#!/usr/bin/env bash
# Rewrite the AUTOBUMP_* sections of dist/homebrew/awidat.rb with
# fresh version + per-platform URLs and SHA256s. Reads the new
# values from env vars (set by .github/workflows/homebrew-bump.yml):
#
#   VERSION           — like 0.2.0 (no leading v)
#   TAG               — like v0.2.0 (used in the URLs)
#   MACOS_ARM_SHA     — sha256 of awidat-aarch64-apple-darwin.tar.gz
#   MACOS_INTEL_SHA   — sha256 of awidat-x86_64-apple-darwin.tar.gz
#   LINUX_ARM_SHA     — sha256 of awidat-aarch64-unknown-linux-gnu.tar.gz
#   LINUX_INTEL_SHA   — sha256 of awidat-x86_64-unknown-linux-gnu.tar.gz
#
# Usage:
#   ./dist/homebrew/bump-formula.sh path/to/awidat.rb > rewritten.rb
#
# Local rehearsal (no real release):
#   VERSION=0.2.0-test TAG=v0.2.0-test \
#     MACOS_ARM_SHA=$(echo -n test1 | shasum -a 256 | cut -d' ' -f1) \
#     MACOS_INTEL_SHA=$(echo -n test2 | shasum -a 256 | cut -d' ' -f1) \
#     LINUX_ARM_SHA=$(echo -n test3 | shasum -a 256 | cut -d' ' -f1) \
#     LINUX_INTEL_SHA=$(echo -n test4 | shasum -a 256 | cut -d' ' -f1) \
#     ./dist/homebrew/bump-formula.sh dist/homebrew/awidat.rb
#
# We use AWK rather than sed because sed's portable subset doesn't
# handle multi-line replacement cleanly; awk's range patterns do.

set -euo pipefail

INPUT="${1:?path/to/awidat.rb required}"
: "${VERSION:?VERSION env var required}"
: "${TAG:?TAG env var required}"
: "${MACOS_ARM_SHA:?MACOS_ARM_SHA env var required}"
: "${MACOS_INTEL_SHA:?MACOS_INTEL_SHA env var required}"
: "${LINUX_ARM_SHA:?LINUX_ARM_SHA env var required}"
: "${LINUX_INTEL_SHA:?LINUX_INTEL_SHA env var required}"

awk -v VERSION="$VERSION" \
    -v TAG="$TAG" \
    -v MACOS_ARM_SHA="$MACOS_ARM_SHA" \
    -v MACOS_INTEL_SHA="$MACOS_INTEL_SHA" \
    -v LINUX_ARM_SHA="$LINUX_ARM_SHA" \
    -v LINUX_INTEL_SHA="$LINUX_INTEL_SHA" '
function emit_version() {
    print "  # AUTOBUMP_VERSION_BEGIN"
    printf "  version \"%s\"\n", VERSION
    print "  # AUTOBUMP_VERSION_END"
}
function emit_block(prefix, indent, triple, sha) {
    printf "%s# AUTOBUMP_%s_BEGIN\n", indent, prefix
    printf "%surl \"https://github.com/explicit09/awidat/releases/download/%s/awidat-%s.tar.gz\"\n", indent, TAG, triple
    printf "%ssha256 \"%s\"\n", indent, sha
    printf "%s# AUTOBUMP_%s_END\n", indent, prefix
}

# Match lines exactly so any reformatting of the template is
# safe (we only care about the markers, not surrounding content).
/^ *# AUTOBUMP_VERSION_BEGIN/ {
    emit_version();
    skip = 1; end_marker = "AUTOBUMP_VERSION_END"; next
}
/^ *# AUTOBUMP_MACOS_ARM_BEGIN/ {
    emit_block("MACOS_ARM", "      ", "aarch64-apple-darwin", MACOS_ARM_SHA);
    skip = 1; end_marker = "AUTOBUMP_MACOS_ARM_END"; next
}
/^ *# AUTOBUMP_MACOS_INTEL_BEGIN/ {
    emit_block("MACOS_INTEL", "      ", "x86_64-apple-darwin", MACOS_INTEL_SHA);
    skip = 1; end_marker = "AUTOBUMP_MACOS_INTEL_END"; next
}
/^ *# AUTOBUMP_LINUX_ARM_BEGIN/ {
    emit_block("LINUX_ARM", "      ", "aarch64-unknown-linux-gnu", LINUX_ARM_SHA);
    skip = 1; end_marker = "AUTOBUMP_LINUX_ARM_END"; next
}
/^ *# AUTOBUMP_LINUX_INTEL_BEGIN/ {
    emit_block("LINUX_INTEL", "      ", "x86_64-unknown-linux-gnu", LINUX_INTEL_SHA);
    skip = 1; end_marker = "AUTOBUMP_LINUX_INTEL_END"; next
}
skip == 1 && $0 ~ ("# " end_marker) { skip = 0; next }
skip == 1 { next }
{ print }
' "$INPUT"
