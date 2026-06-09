# Strict Consumer Installer Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a strict macOS consumer release pipeline that builds signed and notarized Montage DMG installers and publishes them on `v*` tags.

**Architecture:** The release system is made of small testable shell scripts under `scripts/release/`, a focused shell test harness, one GitHub Actions release workflow, and release documentation. GitHub Actions is authoritative for signed release artifacts; local commands are only rehearsals.

**Tech Stack:** Bash, GitHub Actions, Tauri v2, pnpm, Rust/Cargo, Apple Developer ID signing, Apple notarization via `xcrun notarytool`, GitHub Releases.

---

## File Structure

- Create `scripts/release/check-required-env.sh`: validates required environment variables by name.
- Create `scripts/release/verify-sidecars.sh`: validates target-specific Tauri sidecars and rejects CI stubs.
- Create `scripts/release/import-apple-certificate.sh`: imports the base64 Developer ID certificate into a temporary keychain and exports `APPLE_SIGNING_IDENTITY`.
- Create `scripts/release/notarize-dmg.sh`: submits, waits, staples, and validates a DMG.
- Create `scripts/release/checksums.sh`: writes stable SHA-256 files for release artifacts.
- Create `scripts/release/test-release-scripts.sh`: focused shell tests for the helper scripts.
- Create `.github/workflows/release.yml`: builds both macOS DMGs, requires signing/notarization secrets, uploads artifacts, and publishes releases only for `v*` tag pushes.
- Modify `README.md`: add strict consumer installer requirements and local rehearsal commands.
- Modify `CHANGELOG.md`: record the release-pipeline addition under Unreleased.

---

### Task 1: Add Failing Release Script Test Harness

**Files:**
- Create: `scripts/release/test-release-scripts.sh`

- [ ] **Step 1: Write the failing test harness**

Create `scripts/release/test-release-scripts.sh`:

```bash
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

test_verify_sidecars_accepts_real_executables() {
  local dir="$TMP_DIR/real"
  mkdir -p "$dir"
  printf '%s\n' '#!/bin/sh' 'echo codex-real' > "$dir/codex-aarch64-apple-darwin"
  printf '%s\n' '#!/bin/sh' 'echo 2026.03.17' > "$dir/yt-dlp-aarch64-apple-darwin"
  chmod +x "$dir/codex-aarch64-apple-darwin" "$dir/yt-dlp-aarch64-apple-darwin"
  RELEASE_BINARIES_DIR="$dir" bash "$SCRIPT_DIR/verify-sidecars.sh" aarch64-apple-darwin
  pass "real sidecars are accepted"
}

test_checksums_writes_sha256_files() {
  local dir="$TMP_DIR/artifacts"
  mkdir -p "$dir"
  printf 'alpha' > "$dir/Montage_aarch64.dmg"
  printf 'beta' > "$dir/Montage_x64.dmg"
  bash "$SCRIPT_DIR/checksums.sh" "$dir"
  [[ -s "$dir/Montage_aarch64.dmg.sha256" ]] || fail "missing aarch64 checksum"
  [[ -s "$dir/Montage_x64.dmg.sha256" ]] || fail "missing x64 checksum"
  assert_contains "$(cat "$dir/Montage_aarch64.dmg.sha256")" "Montage_aarch64.dmg" "checksum references artifact name"
  pass "checksums are written"
}

test_required_env_reports_missing_name
test_required_env_accepts_present_values
test_verify_sidecars_rejects_missing_files
test_verify_sidecars_rejects_stub
test_verify_sidecars_accepts_real_executables
test_checksums_writes_sha256_files
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
bash scripts/release/test-release-scripts.sh
```

Expected: FAIL because `scripts/release/check-required-env.sh` does not exist.

- [ ] **Step 3: Commit**

Do not commit yet. This task intentionally leaves the red test in the working tree for Task 2.

---

### Task 2: Add Locally Testable Release Helper Scripts

**Files:**
- Create: `scripts/release/check-required-env.sh`
- Create: `scripts/release/verify-sidecars.sh`
- Create: `scripts/release/checksums.sh`
- Modify: `scripts/release/test-release-scripts.sh`

- [ ] **Step 1: Add `check-required-env.sh`**

Create `scripts/release/check-required-env.sh`:

```bash
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
```

- [ ] **Step 2: Add `verify-sidecars.sh`**

Create `scripts/release/verify-sidecars.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 1 ]]; then
  echo "usage: $0 TARGET_TRIPLE" >&2
  exit 2
fi

target="$1"
root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
binaries_dir="${RELEASE_BINARIES_DIR:-$root_dir/apps/desktop/src-tauri/binaries}"

case "$target" in
  aarch64-apple-darwin|x86_64-apple-darwin|x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu)
    codex="$binaries_dir/codex-$target"
    yt_dlp="$binaries_dir/yt-dlp-$target"
    ;;
  x86_64-pc-windows-msvc)
    codex="$binaries_dir/codex-$target.exe"
    yt_dlp="$binaries_dir/yt-dlp-$target.exe"
    ;;
  *)
    echo "unsupported target triple: $target" >&2
    exit 2
    ;;
esac

check_sidecar() {
  local path="$1"
  if [[ ! -e "$path" ]]; then
    echo "missing sidecar: $path" >&2
    exit 1
  fi
  if [[ ! -x "$path" ]]; then
    echo "sidecar is not executable: $path" >&2
    exit 1
  fi
  if grep -aq "sidecar check stub" "$path"; then
    echo "sidecar is a CI check stub, not a release binary: $path" >&2
    exit 1
  fi
}

check_sidecar "$codex"
check_sidecar "$yt_dlp"

echo "release sidecars verified for $target"
```

- [ ] **Step 3: Add `checksums.sh`**

Create `scripts/release/checksums.sh`:

```bash
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
```

- [ ] **Step 4: Make scripts executable**

Run:

```bash
chmod +x scripts/release/check-required-env.sh scripts/release/verify-sidecars.sh scripts/release/checksums.sh scripts/release/test-release-scripts.sh
```

- [ ] **Step 5: Run test to verify it passes**

Run:

```bash
bash scripts/release/test-release-scripts.sh
```

Expected: PASS with six named `ok -` lines.

- [ ] **Step 6: Commit**

Run:

```bash
git add scripts/release/check-required-env.sh scripts/release/verify-sidecars.sh scripts/release/checksums.sh scripts/release/test-release-scripts.sh
git commit -m "test: cover release helper scripts"
```

---

### Task 3: Add Apple Signing And Notarization Scripts

**Files:**
- Create: `scripts/release/import-apple-certificate.sh`
- Create: `scripts/release/notarize-dmg.sh`
- Modify: `scripts/release/test-release-scripts.sh`

- [ ] **Step 1: Extend test harness with usage tests**

Append these functions before the final test calls in `scripts/release/test-release-scripts.sh`:

```bash
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
```

Add these calls at the end:

```bash
test_import_certificate_requires_env
test_notarize_requires_dmg_path
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
bash scripts/release/test-release-scripts.sh
```

Expected: FAIL because `scripts/release/import-apple-certificate.sh` does not exist.

- [ ] **Step 3: Add `import-apple-certificate.sh`**

Create `scripts/release/import-apple-certificate.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
"$script_dir/check-required-env.sh" \
  APPLE_CERTIFICATE \
  APPLE_CERTIFICATE_PASSWORD \
  KEYCHAIN_PASSWORD

work_dir="${RUNNER_TEMP:-$(mktemp -d)}"
cert_path="$work_dir/montage-release-certificate.p12"
keychain_path="$work_dir/montage-release.keychain-db"

printf '%s' "$APPLE_CERTIFICATE" | base64 --decode > "$cert_path"

security create-keychain -p "$KEYCHAIN_PASSWORD" "$keychain_path"
security set-keychain-settings -lut 21600 "$keychain_path"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$keychain_path"
security import "$cert_path" -P "$APPLE_CERTIFICATE_PASSWORD" -A -t cert -f pkcs12 -k "$keychain_path"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$keychain_path"

identity="$(
  security find-identity -v -p codesigning "$keychain_path" \
    | awk -F '"' '/Developer ID Application/ { print $2; exit }'
)"

if [[ -z "$identity" ]]; then
  echo "Developer ID Application signing identity not found in imported certificate" >&2
  exit 1
fi

echo "APPLE_SIGNING_IDENTITY=$identity" >> "${GITHUB_ENV:-/dev/null}"
echo "APPLE_KEYCHAIN_PATH=$keychain_path" >> "${GITHUB_ENV:-/dev/null}"
echo "Developer ID Application signing identity imported"
```

- [ ] **Step 4: Add `notarize-dmg.sh`**

Create `scripts/release/notarize-dmg.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 1 ]]; then
  echo "usage: $0 PATH_TO_DMG" >&2
  exit 2
fi

dmg_path="$1"
if [[ ! -f "$dmg_path" ]]; then
  echo "DMG does not exist: $dmg_path" >&2
  exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
"$script_dir/check-required-env.sh" APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID

xcrun notarytool submit "$dmg_path" \
  --apple-id "$APPLE_ID" \
  --password "$APPLE_PASSWORD" \
  --team-id "$APPLE_TEAM_ID" \
  --wait

xcrun stapler staple "$dmg_path"
xcrun stapler validate "$dmg_path"
spctl --assess --type open --context context:primary-signature -v "$dmg_path"
```

- [ ] **Step 5: Make scripts executable**

Run:

```bash
chmod +x scripts/release/import-apple-certificate.sh scripts/release/notarize-dmg.sh
```

- [ ] **Step 6: Run test to verify it passes**

Run:

```bash
bash scripts/release/test-release-scripts.sh
```

Expected: PASS with eight named `ok -` lines.

- [ ] **Step 7: Commit**

Run:

```bash
git add scripts/release/import-apple-certificate.sh scripts/release/notarize-dmg.sh scripts/release/test-release-scripts.sh
git commit -m "feat: add Apple release signing helpers"
```

---

### Task 4: Add Strict GitHub Release Workflow

**Files:**
- Create: `.github/workflows/release.yml`

- [ ] **Step 1: Add release workflow**

Create `.github/workflows/release.yml`:

```yaml
name: release

on:
  push:
    tags:
      - 'v*'
  workflow_dispatch:

concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false

permissions:
  contents: write

jobs:
  build-macos:
    name: macOS DMG (${{ matrix.target }})
    runs-on: ${{ matrix.runner }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: aarch64-apple-darwin
            runner: macos-latest
            artifact_name: Montage-aarch64-apple-darwin
          - target: x86_64-apple-darwin
            runner: macos-13
            artifact_name: Montage-x86_64-apple-darwin
    env:
      APPLE_ID: ${{ secrets.APPLE_ID }}
      APPLE_PASSWORD: ${{ secrets.APPLE_PASSWORD }}
      APPLE_TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
      APPLE_CERTIFICATE: ${{ secrets.APPLE_CERTIFICATE }}
      APPLE_CERTIFICATE_PASSWORD: ${{ secrets.APPLE_CERTIFICATE_PASSWORD }}
      KEYCHAIN_PASSWORD: ${{ secrets.KEYCHAIN_PASSWORD }}
      CARGO_INCREMENTAL: "0"
      RUSTFLAGS: "-C debuginfo=0"
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - uses: Swatinem/rust-cache@v2

      - uses: pnpm/action-setup@v4
        with:
          package_json_file: apps/desktop/package.json
          run_install: false

      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: pnpm
          cache-dependency-path: apps/desktop/pnpm-lock.yaml

      - name: Refuse missing release secrets
        run: |
          scripts/release/check-required-env.sh \
            APPLE_ID \
            APPLE_PASSWORD \
            APPLE_TEAM_ID \
            APPLE_CERTIFICATE \
            APPLE_CERTIFICATE_PASSWORD \
            KEYCHAIN_PASSWORD

      - name: Install desktop dependencies
        run: pnpm --dir apps/desktop install --frozen-lockfile

      - name: Fetch real release sidecars
        run: |
          make desktop-yt-dlp TARGET_TRIPLE=${{ matrix.target }}
          make desktop-codex TARGET_TRIPLE=${{ matrix.target }}

      - name: Verify release sidecars
        run: scripts/release/verify-sidecars.sh ${{ matrix.target }}

      - name: Import Apple Developer ID certificate
        run: scripts/release/import-apple-certificate.sh

      - name: Build signed DMG
        env:
          APPLE_SIGNING_IDENTITY: ${{ env.APPLE_SIGNING_IDENTITY }}
        run: pnpm --dir apps/desktop tauri build --target ${{ matrix.target }} --bundles dmg

      - name: Stage DMG
        run: |
          set -euo pipefail
          mkdir -p release-artifacts
          dmg="$(find apps/desktop/src-tauri/target/${{ matrix.target }}/release/bundle/dmg -maxdepth 1 -name '*.dmg' -print -quit)"
          if [ -z "$dmg" ]; then
            echo "expected DMG artifact was not produced" >&2
            exit 1
          fi
          cp "$dmg" "release-artifacts/${{ matrix.artifact_name }}.dmg"

      - name: Notarize and staple DMG
        run: scripts/release/notarize-dmg.sh "release-artifacts/${{ matrix.artifact_name }}.dmg"

      - name: Compute checksums
        run: scripts/release/checksums.sh release-artifacts

      - uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.artifact_name }}
          path: release-artifacts/*
          retention-days: 14

  publish:
    name: publish GitHub release
    needs: build-macos
    runs-on: ubuntu-latest
    if: startsWith(github.ref, 'refs/tags/v')
    steps:
      - uses: actions/download-artifact@v4
        with:
          path: artifacts

      - name: Stage release assets
        run: |
          set -euo pipefail
          mkdir -p release-assets
          find artifacts -type f -name '*.dmg' -exec cp {} release-assets/ \;
          find artifacts -type f -name '*.sha256' -exec cp {} release-assets/ \;
          if ! ls release-assets/*.dmg >/dev/null 2>&1; then
            echo "no DMG files downloaded" >&2
            exit 1
          fi
          (cd release-assets && cat ./*.sha256 > checksums.txt)
          ls -la release-assets

      - uses: softprops/action-gh-release@v2
        with:
          files: release-assets/*
          draft: false
          prerelease: ${{ contains(github.ref_name, '-') }}
          generate_release_notes: true
          fail_on_unmatched_files: true
```

- [ ] **Step 2: Run static checks**

Run:

```bash
git diff --check .github/workflows/release.yml
```

Expected: no output, exit 0.

- [ ] **Step 3: Validate workflow syntax when `gh` is available**

Run:

```bash
gh workflow list --all
```

Expected: existing workflows list successfully. The new workflow will not appear until pushed, so this only verifies GitHub CLI access.

- [ ] **Step 4: Commit**

Run:

```bash
git add .github/workflows/release.yml
git commit -m "ci: add strict macOS release workflow"
```

---

### Task 5: Add Release Documentation

**Files:**
- Modify: `README.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Update README release section**

In `README.md`, replace the sentence that says consumer installers are not ready with:

```markdown
This repository is a developer-preview source release. It is intended for
contributors who can build and run the project from source. The macOS consumer
installer track now builds signed and notarized DMGs from GitHub Actions on
`v*` tags; Linux packages, Windows installers, Homebrew publishing, auto-update,
and broader bundled runtime polish remain future release work.
```

Add this section after the desktop development section:

```markdown
## macOS consumer releases

Strict macOS consumer releases are built by `.github/workflows/release.yml`.
The workflow runs on `v*` tags and creates notarized DMGs for:

- `aarch64-apple-darwin`
- `x86_64-apple-darwin`

Required GitHub Actions secrets:

- `APPLE_ID`
- `APPLE_PASSWORD`
- `APPLE_TEAM_ID`
- `APPLE_CERTIFICATE`
- `APPLE_CERTIFICATE_PASSWORD`
- `KEYCHAIN_PASSWORD`

`APPLE_CERTIFICATE` must be a base64-encoded Developer ID Application `.p12`.
`APPLE_PASSWORD` should be an Apple app-specific password with notarization
access for `APPLE_TEAM_ID`.

Local rehearsal for the current Mac target:

```sh
make desktop-yt-dlp
make desktop-codex
scripts/release/verify-sidecars.sh "$(rustc -vV | awk '/^host:/ { print $2 }')"
pnpm --dir apps/desktop tauri build --bundles dmg
```

CI release builds are strict: missing Apple secrets, stub sidecars, failed
signing, failed notarization, or failed stapling all fail the release.
```

- [ ] **Step 2: Update changelog**

In `CHANGELOG.md`, under `## Unreleased`, add:

```markdown
- Added a strict macOS consumer release path for signed and notarized Tauri DMG artifacts on `v*` tags.
```

- [ ] **Step 3: Run documentation checks**

Run:

```bash
rg -n "consumer releases|APPLE_CERTIFICATE|release.yml|notarized DMGs" README.md CHANGELOG.md
git diff --check README.md CHANGELOG.md
```

Expected: the new release documentation lines appear and `git diff --check` exits 0.

- [ ] **Step 4: Commit**

Run:

```bash
git add README.md CHANGELOG.md
git commit -m "docs: document macOS consumer releases"
```

---

### Task 6: Final Local Verification And Release Readiness Report

**Files:**
- No source edits expected.

- [ ] **Step 1: Run release script tests**

Run:

```bash
bash scripts/release/test-release-scripts.sh
```

Expected: all release helper script tests pass.

- [ ] **Step 2: Run whitespace check**

Run:

```bash
git diff --check origin/main..HEAD
```

Expected: no output, exit 0.

- [ ] **Step 3: Confirm workflow and docs exist**

Run:

```bash
test -f .github/workflows/release.yml
test -x scripts/release/check-required-env.sh
test -x scripts/release/verify-sidecars.sh
test -x scripts/release/import-apple-certificate.sh
test -x scripts/release/notarize-dmg.sh
test -x scripts/release/checksums.sh
rg -n "macOS consumer releases|APPLE_CERTIFICATE|notarized DMGs" README.md
```

Expected: all `test` commands pass and `rg` prints matching README lines.

- [ ] **Step 4: Confirm branch state**

Run:

```bash
git status --short --branch
git log --oneline --max-count=6
```

Expected: branch is ahead of `origin/main` by the implementation commits and has no unstaged files.

- [ ] **Step 5: Report external verification still required**

Do not claim that signed releases are proven until one of these has run:

```bash
gh workflow run release.yml
```

or a real tag push:

```bash
git tag v0.1.0-rc.1
git push origin v0.1.0-rc.1
```

Expected: GitHub Actions either produces notarized DMGs or fails with a specific Apple signing/notarization error.
