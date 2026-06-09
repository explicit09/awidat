#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT_DIR="$ROOT_DIR/scripts/release"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

pass() {
  printf 'ok - %s\n' "$1"
}

fail() {
  printf 'not ok - %s\n' "$1" >&2
  exit 1
}

assert_contains() {
  local haystack="$1"
  local needle="$2"
  local label="$3"
  if [[ "$haystack" != *"$needle"* ]]; then
    printf 'expected output to contain %q\nactual output:\n%s\n' "$needle" "$haystack" >&2
    fail "$label"
  fi
}

assert_equals() {
  local actual="$1"
  local expected="$2"
  local label="$3"
  if [[ "$actual" != "$expected" ]]; then
    printf 'expected: %s\nactual:   %s\n' "$expected" "$actual" >&2
    fail "$label"
  fi
}

test_required_env_reports_missing_name() {
  local output
  if output="$(env -i bash "$SCRIPT_DIR/check-required-env.sh" APPLE_ID APPLE_PASSWORD 2>&1)"; then
    fail "missing env test should fail"
  fi
  assert_contains "$output" "missing required environment variable: APPLE_ID" "missing env names first missing variable"
  pass "missing env names first missing variable"
}

test_required_env_accepts_present_values() {
  APPLE_ID="dev@example.com" APPLE_PASSWORD="app-password" \
    bash "$SCRIPT_DIR/check-required-env.sh" APPLE_ID APPLE_PASSWORD
  pass "required env accepts present values"
}

test_verify_sidecars_rejects_missing_files() {
  local output
  if output="$(RELEASE_BINARIES_DIR="$TMP_DIR/missing" bash "$SCRIPT_DIR/verify-sidecars.sh" aarch64-apple-darwin 2>&1)"; then
    fail "missing sidecars should fail"
  fi
  assert_contains "$output" "missing sidecar" "missing sidecars report missing file"
  pass "missing sidecars report missing file"
}

test_verify_sidecars_rejects_stub() {
  local dir="$TMP_DIR/stub"
  mkdir -p "$dir"
  printf '%s\n' '#!/bin/sh' 'echo "codex sidecar check stub; run make desktop-codex for a runnable sidecar" >&2' 'exit 127' > "$dir/codex-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo "2026.03.17"' > "$dir/yt-dlp-aarch64-apple-darwin"
  chmod +x "$dir/codex-aarch64-apple-darwin" "$dir/yt-dlp-aarch64-apple-darwin"
  local output
  if output="$(RELEASE_BINARIES_DIR="$dir" bash "$SCRIPT_DIR/verify-sidecars.sh" aarch64-apple-darwin 2>&1)"; then
    fail "stub sidecar should fail"
  fi
  assert_contains "$output" "sidecar is a CI check stub" "stub sidecar is rejected"
  pass "stub sidecar is rejected"
}

test_verify_sidecars_rejects_yt_dlp_ci_placeholder() {
  local dir="$TMP_DIR/yt-dlp-placeholder"
  mkdir -p "$dir"
  printf '%s\n' '#!/bin/sh' 'echo codex-real' > "$dir/codex-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo "yt-dlp sidecar unavailable in CI compile check" >&2' 'exit 127' > "$dir/yt-dlp-aarch64-apple-darwin"
  chmod +x "$dir/codex-aarch64-apple-darwin" "$dir/yt-dlp-aarch64-apple-darwin"
  local output
  if output="$(RELEASE_BINARIES_DIR="$dir" bash "$SCRIPT_DIR/verify-sidecars.sh" aarch64-apple-darwin 2>&1)"; then
    fail "yt-dlp CI placeholder should fail"
  fi
  assert_contains "$output" "sidecar is a CI check stub" "yt-dlp CI placeholder is rejected"
  pass "yt-dlp CI placeholder is rejected"
}

test_verify_sidecars_accepts_executable_non_stub_files() {
  local dir="$TMP_DIR/real"
  mkdir -p "$dir"
  printf '%s\n' '#!/bin/sh' 'echo codex-real' > "$dir/codex-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo 2026.03.17' > "$dir/yt-dlp-aarch64-apple-darwin"
  chmod +x "$dir/codex-aarch64-apple-darwin" "$dir/yt-dlp-aarch64-apple-darwin"
  RELEASE_BINARIES_DIR="$dir" bash "$SCRIPT_DIR/verify-sidecars.sh" aarch64-apple-darwin
  pass "executable non-stub sidecars are accepted"
}

test_checksums_writes_sha256_files() {
  local dir="$TMP_DIR/artifacts"
  mkdir -p "$dir"
  printf 'alpha' > "$dir/Montage_aarch64.dmg"
  printf 'beta' > "$dir/Montage_x64.dmg"
  bash "$SCRIPT_DIR/checksums.sh" "$dir"
  [[ -s "$dir/Montage_aarch64.dmg.sha256" ]] || fail "missing aarch64 checksum"
  [[ -s "$dir/Montage_x64.dmg.sha256" ]] || fail "missing x64 checksum"
  assert_equals "$(cat "$dir/Montage_aarch64.dmg.sha256")" "8ed3f6ad685b959ead7022518e1af76cd816f8e8ec7ccdda1ed4018e8f2223f8  Montage_aarch64.dmg" "aarch64 checksum line matches fixture digest"
  assert_equals "$(cat "$dir/Montage_x64.dmg.sha256")" "f44e64e75f3948e9f73f8dfa94721c4ce8cbb4f265c4790c702b2d41cfbf2753  Montage_x64.dmg" "x64 checksum line matches fixture digest"
  pass "checksums are written"
}

test_import_certificate_requires_env() {
  local output
  if output="$(env -i bash "$SCRIPT_DIR/import-apple-certificate.sh" 2>&1)"; then
    fail "certificate import should require env"
  fi
  assert_contains "$output" "missing required environment variable: APPLE_CERTIFICATE" "certificate import requires env"
  pass "certificate import requires env"
}

test_notarize_requires_dmg_path() {
  local output
  if output="$(bash "$SCRIPT_DIR/notarize-dmg.sh" 2>&1)"; then
    fail "notarize usage should fail without path"
  fi
  assert_contains "$output" "usage:" "notarize script prints usage"
  pass "notarize script prints usage"
}

test_required_env_reports_missing_name
test_required_env_accepts_present_values
test_verify_sidecars_rejects_missing_files
test_verify_sidecars_rejects_stub
test_verify_sidecars_rejects_yt_dlp_ci_placeholder
test_verify_sidecars_accepts_executable_non_stub_files
test_checksums_writes_sha256_files
test_import_certificate_requires_env
test_notarize_requires_dmg_path
