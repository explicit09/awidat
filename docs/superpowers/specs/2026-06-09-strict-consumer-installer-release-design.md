# Strict Consumer Installer Release Design

## Goal

Create a strict consumer release system for Montage that produces signed and notarized macOS DMG installers from GitHub Actions and refuses to publish release artifacts when Apple signing or notarization is not configured correctly.

## Current State

Current `main` has Tauri bundling enabled in `apps/desktop/src-tauri/tauri.conf.json`, plus `Makefile` targets that can fetch or build the required `yt-dlp` and `codex` sidecars for a target triple. It does not have a release workflow, release packaging scripts, signing/notarization setup, or GitHub Release artifact publication.

The README and changelog correctly describe consumer installers as deferred work. This release track changes that for macOS by making DMG creation repeatable and gated by signing/notarization.

## Scope

This first consumer-installer slice ships:

- Apple Silicon DMG: `aarch64-apple-darwin`
- Intel macOS DMG: `x86_64-apple-darwin`
- target-specific `codex` and `yt-dlp` sidecars bundled into each app
- Apple Developer ID signing
- Apple notarization and stapling
- SHA-256 checksums
- GitHub Release upload on `v*` tags
- manual `workflow_dispatch` rehearsal support that builds and validates artifacts without publishing a real release

Linux packages, Windows installers, Homebrew formula updates, and auto-update feeds are follow-up work. They are excluded from this slice and do not block strict macOS consumer installers.

## Required Release Inputs

GitHub Actions must fail early if any required Apple or release input is missing:

- `APPLE_ID`
- `APPLE_PASSWORD`
- `APPLE_TEAM_ID`
- `APPLE_CERTIFICATE`
- `APPLE_CERTIFICATE_PASSWORD`
- `KEYCHAIN_PASSWORD`

`APPLE_CERTIFICATE` is a base64-encoded Developer ID Application `.p12` certificate. `APPLE_PASSWORD` is an app-specific password for the Apple ID that has notarization access for `APPLE_TEAM_ID`.

Local release rehearsals may use the developer's terminal-authenticated Apple setup, but the CI release path is authoritative and must not rely on a local login.

## Architecture

Add a release workflow at `.github/workflows/release.yml` with a macOS matrix for:

- `aarch64-apple-darwin` on `macos-latest`
- `x86_64-apple-darwin` on `macos-13` or another Intel-capable runner

Each matrix job performs the same sequence:

1. Check out the repository.
2. Install Rust, Node, and pnpm using the repo's existing desktop CI pattern.
3. Install desktop dependencies with `pnpm --dir apps/desktop install --frozen-lockfile`.
4. Fetch the real sidecars:
   - `make desktop-yt-dlp TARGET_TRIPLE=<target>`
   - `make desktop-codex TARGET_TRIPLE=<target>`
5. Verify sidecars exist, are executable, and are not CI stubs.
6. Import the Developer ID certificate into a temporary keychain.
7. Build a signed Tauri DMG:
   - `pnpm --dir apps/desktop tauri build --target <target> --bundles dmg`
8. Notarize and staple the DMG.
9. Verify notarization/stapling.
10. Compute SHA-256 checksums.
11. Upload build artifacts.

A publish job runs only for real `v*` tag pushes. It downloads the notarized DMGs and checksum files, creates a combined `checksums.txt`, and publishes a GitHub Release.

## Release Scripts

Use small shell scripts under `scripts/release/` to keep the workflow readable and testable:

- `check-required-env.sh`: exits with a clear error if required variables are missing.
- `verify-sidecars.sh <target>`: checks for target-specific `codex` and `yt-dlp` binaries and rejects the known CI stub text.
- `import-apple-certificate.sh`: imports the base64 `.p12` certificate into a temporary keychain and writes the signing identity to `GITHUB_ENV`.
- `notarize-dmg.sh <dmg-path>`: submits the DMG to Apple notarization, waits for completion, staples the result, and validates stapling.
- `checksums.sh <artifact-dir>`: writes per-artifact `.sha256` files with stable paths.

These scripts use `set -euo pipefail`, avoid secrets in logs, and produce actionable error messages.

## Error Handling

The release workflow must fail, not degrade, when:

- Apple secrets are missing.
- the signing certificate cannot be imported.
- the signing identity cannot be found.
- sidecars are absent or are CI stubs.
- Tauri build fails.
- notarization fails.
- stapling validation fails.
- expected DMG artifacts are missing.

The only allowed non-publishing path is `workflow_dispatch`, which can build and validate artifacts but must not publish a GitHub Release unless it is running on a real `v*` tag.

## Testing And Verification

Add focused tests for the release helper scripts before implementation:

- missing required environment variables fail with the missing variable name.
- all required variables present passes.
- missing sidecars fail.
- executable sidecars pass.
- sidecar files containing the CI stub marker fail.
- checksum generation creates `.sha256` files for provided artifacts.

Verification commands:

- `bash scripts/release/check-required-env.sh APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD KEYCHAIN_PASSWORD` through the focused test harness.
- `bash scripts/release/verify-sidecars.sh aarch64-apple-darwin` through fixture directories.
- `bash scripts/release/checksums.sh <fixture-dir>` through fixture artifacts.
- `git diff --check`.
- A real workflow run once secrets are configured.

## Success Criteria

The work is complete when:

- `.github/workflows/release.yml` exists and is wired to `v*` tag pushes plus manual dispatch.
- release helper scripts exist and have focused tests.
- release workflow builds both macOS target DMGs with real sidecars.
- workflow fails if Apple signing/notarization inputs are absent.
- notarized DMGs and checksums are uploaded to GitHub Releases for `v*` tags.
- docs list required secrets and the local rehearsal command.

## Follow-Up Scope

After macOS consumer DMGs are strict and repeatable:

- add Homebrew formula publishing.
- add Linux packages.
- add Windows installer/signing.
- add Tauri updater feed/signatures.
- add automated launch smoke tests against installed artifacts.
