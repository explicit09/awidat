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

assert_not_contains() {
  local haystack="$1"
  local needle="$2"
  local label="$3"
  if [[ "$haystack" == *"$needle"* ]]; then
    printf 'expected output not to contain %q\nactual output:\n%s\n' "$needle" "$haystack" >&2
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
  printf '%s\n' '#!/bin/sh' 'echo ffmpeg-real' > "$dir/ffmpeg-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo ffprobe-real' > "$dir/ffprobe-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo mcp-real' > "$dir/montage-mcp-server-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo uv-real' > "$dir/uv-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo "yt-dlp sidecar unavailable in CI compile check" >&2' 'exit 127' > "$dir/yt-dlp-aarch64-apple-darwin"
  chmod +x "$dir"/*-aarch64-apple-darwin
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
  printf '%s\n' '#!/bin/sh' 'echo ffmpeg-real' > "$dir/ffmpeg-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo ffprobe-real' > "$dir/ffprobe-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo mcp-real' > "$dir/montage-mcp-server-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo uv-real' > "$dir/uv-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo 2026.03.17' > "$dir/yt-dlp-aarch64-apple-darwin"
  chmod +x "$dir"/*-aarch64-apple-darwin
  RELEASE_BINARIES_DIR="$dir" bash "$SCRIPT_DIR/verify-sidecars.sh" aarch64-apple-darwin
  pass "executable non-stub sidecars are accepted"
}

test_verify_sidecars_rejects_missing_required_uv() {
  local dir="$TMP_DIR/missing-uv"
  mkdir -p "$dir"
  printf '%s\n' '#!/bin/sh' 'echo codex-real' > "$dir/codex-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo ffmpeg-real' > "$dir/ffmpeg-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo ffprobe-real' > "$dir/ffprobe-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo mcp-real' > "$dir/montage-mcp-server-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo 2026.03.17' > "$dir/yt-dlp-aarch64-apple-darwin"
  chmod +x "$dir"/*-aarch64-apple-darwin

  local output
  if output="$(RELEASE_BINARIES_DIR="$dir" bash "$SCRIPT_DIR/verify-sidecars.sh" aarch64-apple-darwin 2>&1)"; then
    fail "missing uv sidecar should fail"
  fi
  assert_contains "$output" "missing sidecar" "missing uv sidecar is reported"
  assert_contains "$output" "uv-aarch64-apple-darwin" "missing uv path is named"
  pass "missing uv sidecar is rejected"
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

test_make_desktop_ffmpeg_supports_linux_targets() {
  local output
  output="$(make -C "$ROOT_DIR" -n desktop-ffmpeg TARGET_TRIPLE=x86_64-unknown-linux-gnu)"
  assert_contains "$output" "fetch_npm_sidecars linux-x64 4.1.0 linux-x64 5.2.0" "linux x64 ffmpeg and ffprobe tarballs are selected"

  output="$(make -C "$ROOT_DIR" -n desktop-ffmpeg TARGET_TRIPLE=aarch64-unknown-linux-gnu)"
  assert_contains "$output" "fetch_npm_sidecars linux-arm64 4.1.4 linux-arm64 5.2.0" "linux arm64 ffmpeg and ffprobe tarballs are selected"
  pass "desktop ffmpeg supports linux targets"
}

test_make_desktop_ffmpeg_uses_arm64_darwin_artifacts() {
  local output
  output="$(make -C "$ROOT_DIR" -n desktop-ffmpeg TARGET_TRIPLE=aarch64-apple-darwin)"
  assert_contains "$output" "fetch_npm_sidecars darwin-arm64 4.1.5 darwin-arm64 5.0.1" "darwin arm64 ffmpeg and ffprobe tarballs are selected"
  assert_contains "$output" 'sidecar check stub|sidecar unavailable in CI compile check' "ffmpeg skips reject CI stubs"
  pass "desktop ffmpeg uses arm64 darwin artifacts"
}

test_make_desktop_mcp_server_builds_requested_target() {
  local output
  output="$(make -C "$ROOT_DIR" -n desktop-mcp-server TARGET_TRIPLE=x86_64-apple-darwin)"
  assert_contains "$output" '--target "$target_triple"' "mcp sidecar build passes target triple"
  assert_contains "$output" 'source="$cargo_target_dir/$target_triple/release/montage-mcp-server"' "mcp sidecar copies target-qualified binary"
  pass "desktop mcp server builds requested target"
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

test_import_certificate_imports_identity_with_fake_security() {
  local fake_bin="$TMP_DIR/fake-security-bin"
  local runner_temp="$TMP_DIR/runner-temp"
  local github_env="$TMP_DIR/github-env"
  local security_log="$TMP_DIR/security.log"
  mkdir -p "$fake_bin" "$runner_temp"

  cat > "$fake_bin/security" <<EOF
#!/usr/bin/env bash
printf '%s\\n' "\$*" >> "$security_log"
if [[ "\$1" == "find-identity" ]]; then
  printf '  1) ABCDEF1234567890 "Developer ID Application: Montage Test (TEAMID)"\\n'
fi
EOF
  chmod +x "$fake_bin/security"

  APPLE_CERTIFICATE="$(printf 'fake certificate content' | base64)" \
    APPLE_CERTIFICATE_PASSWORD="cert-password" \
    KEYCHAIN_PASSWORD="keychain-password" \
    RUNNER_TEMP="$runner_temp" \
    GITHUB_ENV="$github_env" \
    PATH="$fake_bin:/usr/bin:/bin:/usr/sbin:/sbin" \
    bash "$SCRIPT_DIR/import-apple-certificate.sh"

  assert_contains "$(cat "$github_env")" "APPLE_SIGNING_IDENTITY=Developer ID Application: Montage Test (TEAMID)" "certificate import writes signing identity"
  assert_contains "$(cat "$github_env")" "APPLE_KEYCHAIN_PATH=" "certificate import writes keychain path"
  assert_contains "$(cat "$github_env")" "APPLE_KEYCHAIN_PATH=$runner_temp/montage-release." "certificate import uses unique runner temp subdirectory"
  assert_contains "$(cat "$security_log")" "create-keychain" "certificate import creates keychain"
  assert_contains "$(cat "$security_log")" "import" "certificate import imports certificate"
  assert_contains "$(cat "$security_log")" "find-identity" "certificate import finds identity"
  assert_not_contains "$(cat "$security_log")" " -A " "certificate import avoids broad key access"
  if find "$runner_temp" -name '*.p12' -print -quit | grep -q .; then
    fail "certificate import removes decoded certificate"
  fi
  pass "certificate import succeeds with fake security"
}

test_import_certificate_prints_local_exports_without_github_env() {
  local fake_bin="$TMP_DIR/fake-local-security-bin"
  local runner_temp="$TMP_DIR/local-runner-temp"
  local security_log="$TMP_DIR/local-security.log"
  local output
  mkdir -p "$fake_bin" "$runner_temp"

  cat > "$fake_bin/security" <<EOF
#!/usr/bin/env bash
printf '%s\\n' "\$*" >> "$security_log"
if [[ "\$1" == "find-identity" ]]; then
  printf '  1) ABCDEF1234567890 "Developer ID Application: Montage Test (TEAMID)"\\n'
fi
EOF
  chmod +x "$fake_bin/security"

  output="$(
    APPLE_CERTIFICATE="$(printf 'fake certificate content' | base64)" \
      APPLE_CERTIFICATE_PASSWORD="cert-password" \
      KEYCHAIN_PASSWORD="keychain-password" \
      RUNNER_TEMP="$runner_temp" \
      PATH="$fake_bin:/usr/bin:/bin:/usr/sbin:/sbin" \
      bash "$SCRIPT_DIR/import-apple-certificate.sh"
  )"

  assert_contains "$output" "export APPLE_SIGNING_IDENTITY=Developer\\ ID\\ Application:\\ Montage\\ Test\\ \\(TEAMID\\)" "certificate import prints local signing identity export"
  assert_contains "$output" "export APPLE_KEYCHAIN_PATH=" "certificate import prints local keychain export"
  assert_contains "$output" "Developer ID Application signing identity imported" "certificate import still prints status"
  pass "certificate import prints local exports"
}

test_notarize_runs_apple_tools_with_fake_commands() {
  local fake_bin="$TMP_DIR/fake-notary-bin"
  local notary_log="$TMP_DIR/notary.log"
  local dmg_path="$TMP_DIR/Montage-test.dmg"
  mkdir -p "$fake_bin"
  printf 'fake dmg' > "$dmg_path"

  cat > "$fake_bin/xcrun" <<EOF
#!/usr/bin/env bash
printf 'xcrun %s\\n' "\$*" >> "$notary_log"
EOF
  cat > "$fake_bin/spctl" <<EOF
#!/usr/bin/env bash
printf 'spctl %s\\n' "\$*" >> "$notary_log"
EOF
  chmod +x "$fake_bin/xcrun" "$fake_bin/spctl"

  APPLE_ID="dev@example.com" \
    APPLE_PASSWORD="app-password" \
    APPLE_TEAM_ID="TEAMID" \
    PATH="$fake_bin:/usr/bin:/bin:/usr/sbin:/sbin" \
    bash "$SCRIPT_DIR/notarize-dmg.sh" "$dmg_path"

  assert_contains "$(cat "$notary_log")" "xcrun notarytool submit" "notarize submits dmg"
  assert_contains "$(cat "$notary_log")" "xcrun stapler staple" "notarize staples dmg"
  assert_contains "$(cat "$notary_log")" "xcrun stapler validate" "notarize validates staple"
  assert_contains "$(cat "$notary_log")" "spctl --assess" "notarize assesses dmg"
  assert_equals "$(sed -n '1p' "$notary_log")" "xcrun notarytool submit $dmg_path --apple-id dev@example.com --password app-password --team-id TEAMID --wait" "notarize submits before stapling"
  assert_equals "$(sed -n '2p' "$notary_log")" "xcrun stapler staple $dmg_path" "notarize staples before validate"
  assert_equals "$(sed -n '3p' "$notary_log")" "xcrun stapler validate $dmg_path" "notarize validates before assess"
  assert_contains "$(sed -n '4p' "$notary_log")" "spctl --assess --type open --context context:primary-signature -v $dmg_path" "notarize assesses last"
  pass "notarize runs Apple tools with fake commands"
}

test_required_env_reports_missing_name
test_required_env_accepts_present_values
test_verify_sidecars_rejects_missing_files
test_verify_sidecars_rejects_stub
test_verify_sidecars_rejects_yt_dlp_ci_placeholder
test_verify_sidecars_accepts_executable_non_stub_files
test_verify_sidecars_rejects_missing_required_uv
test_checksums_writes_sha256_files
test_make_desktop_ffmpeg_supports_linux_targets
test_make_desktop_ffmpeg_uses_arm64_darwin_artifacts
test_make_desktop_mcp_server_builds_requested_target
test_import_certificate_requires_env
test_notarize_requires_dmg_path
test_import_certificate_imports_identity_with_fake_security
test_import_certificate_prints_local_exports_without_github_env
test_notarize_runs_apple_tools_with_fake_commands
