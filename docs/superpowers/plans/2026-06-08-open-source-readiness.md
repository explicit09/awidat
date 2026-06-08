# Awidat Open-Source Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Awidat/Montage repository safe to publish as a developer-preview open-source project before doing consumer installer work.

**Architecture:** This plan treats open-source readiness as a sequence of hard gates. Complete exactly one task at a time, commit it, run its verification gate, and do not start the next task until the current task is proven complete. Consumer distribution items such as notarization, Windows signing, updater, ffmpeg bundling, and Python bundling are documented but intentionally deferred because they are release-readiness work, not source-publication blockers.

**Tech Stack:** Rust workspace, Tauri desktop app, shell scripts, GitHub Actions, Markdown documentation, repository hygiene checks.

---

## Execution Rules

- Worktree: `/Users/explicit/.config/superpowers/worktrees/awidat/open-source-readiness`
- Branch: `codex/open-source-readiness`
- Complete tasks in numeric order.
- Do not edit the next task's files until the current task's verification gate passes.
- Each task must end with one focused commit.
- If a verification gate fails, fix only the current task until it passes.
- Preserve unrelated worktree changes. If new unrelated changes appear, stop and identify them before continuing.

## Source Findings To Resolve

- OpenAI/Codex policy exposure: hardcoded Codex version stamps and first-party OAuth client reuse.
- Repo hygiene exposure: committed local AEAD fallback key, predictable bearer-token defaults, and Supabase project ref.
- Public data-flow gap: no privacy policy or data-egress disclosure.
- Third-party notice gap: missing notices for model/provider surfaces and binary/media provenance.
- Contributor hygiene gap: README/community links, DCO/PR template, changelog, architecture guide, support expectations.
- CI hygiene gap: no root dependency/license/advisory policy and no secret scanning.
- Binary hygiene gap: no `.gitattributes`; office documents and bytecode are not broadly ignored.

---

### Task 0: Baseline Receipt

**Files:**
- Create: `docs/superpowers/plans/2026-06-08-open-source-readiness-baseline.md`

- [ ] **Step 1: Record worktree and branch**

Run:

```bash
pwd
git branch --show-current
git status --short
```

Expected:

```text
/Users/explicit/.config/superpowers/worktrees/awidat/open-source-readiness
codex/open-source-readiness
```

`git status --short` may show this plan file before Task 0 is committed. It must not show unrelated modified tracked files.

- [ ] **Step 2: Record current blockers**

Run:

```bash
{
  echo "# Open-Source Readiness Baseline"
  echo
  echo "Date: 2026-06-08"
  echo "Branch: $(git branch --show-current)"
  echo
  echo "## Evidence"
  echo
  echo '```text'
  rg -n "0\\.128\\.0|first-party OAuth|SOCIAL_TOKEN_AEAD_KEY|vgkocfbtkzmpklruqmsx" \
    vendor/codex-rs/SOURCE \
    vendor/codex-rs/login/src/auth/default_client.rs \
    crates/auth/src/lib.rs \
    crates/social-server/run-local.sh \
    docs/social-server/README.md
  echo '```'
} > docs/superpowers/plans/2026-06-08-open-source-readiness-baseline.md
```

- [ ] **Step 3: Commit baseline artifacts**

Run:

```bash
git add docs/superpowers/plans/2026-06-08-open-source-readiness.md docs/superpowers/plans/2026-06-08-open-source-readiness-baseline.md
git commit -m "docs: plan open-source readiness sequence"
```

- [ ] **Task 0 verification gate**

Run:

```bash
git status --short
git log --oneline -1
test -f docs/superpowers/plans/2026-06-08-open-source-readiness.md
test -f docs/superpowers/plans/2026-06-08-open-source-readiness-baseline.md
```

Expected:

- `git status --short` is empty.
- Latest commit is `docs: plan open-source readiness sequence`.
- Both plan files exist.

---

### Task 1: Remove Unsanctioned OpenAI/Codex Public Defaults

**Files:**
- Modify: `crates/auth/src/lib.rs`
- Modify: `vendor/codex-rs/login/src/auth/default_client.rs`
- Modify: `vendor/codex-rs/models-manager/src/lib.rs`
- Modify: `vendor/codex-rs/exec/src/lib.rs`
- Modify: `vendor/codex-rs/exec/src/event_processor_with_human_output.rs`
- Modify: `vendor/codex-rs/analytics/src/events.rs`
- Modify: `vendor/codex-rs/model-provider-info/src/lib.rs`
- Modify: `vendor/codex-rs/codex-api/src/requests/headers.rs`
- Modify: `vendor/codex-rs/SOURCE`
- Test: existing tests in `crates/auth/src/lib.rs`

- [ ] **Step 1: Write/adjust auth tests first**

Update the `oauth_client_id_default_override_and_blank` test in `crates/auth/src/lib.rs` so the expected public default is no built-in Codex first-party client. The test must prove:

- unset `MONTAGE_OAUTH_CLIENT_ID` returns an error or disabled state, not `codex_login::CLIENT_ID`;
- blank `MONTAGE_OAUTH_CLIENT_ID` returns the same disabled state;
- a non-empty override is accepted.

- [ ] **Step 2: Run the targeted auth test and verify it fails**

Run:

```bash
cargo test -p montage-auth oauth_client_id_default_override_and_blank
```

Expected: FAIL because current code falls back to `codex_login::CLIENT_ID`.

- [ ] **Step 3: Remove first-party OAuth fallback**

Change `oauth_client_id()` so public source defaults do not silently reuse `codex_login::CLIENT_ID`. The smallest acceptable behavior is:

- return a typed error when `MONTAGE_OAUTH_CLIENT_ID` is unset or blank;
- allow explicit `MONTAGE_OAUTH_CLIENT_ID` overrides;
- keep API-key auth usable.

Update direct callers to handle the typed error with a clear message:

```text
ChatGPT OAuth is not configured for this build. Set MONTAGE_OAUTH_CLIENT_ID to a sanctioned client id or use API-key auth.
```

- [ ] **Step 4: Remove hardcoded Codex version spoofing**

Replace Montage-added hardcoded `"0.128.0"` request/version stamps with the package version or a clearly Montage-owned version value. Update `vendor/codex-rs/SOURCE` to say the prior spoofing was removed for open-source publication, and that model availability now depends on sanctioned API/OAuth access.

- [ ] **Step 5: Run targeted verification**

Run:

```bash
cargo test -p montage-auth
rg -n "app_EMoamEEZ73f0CkXaXp7hrann|0\\.128\\.0|requires a newer version|installed Codex 0\\.128\\.0|first-party client id" \
  crates/auth vendor/codex-rs/SOURCE vendor/codex-rs/login/src/auth vendor/codex-rs/models-manager/src vendor/codex-rs/exec/src vendor/codex-rs/analytics/src vendor/codex-rs/model-provider-info/src vendor/codex-rs/codex-api/src || true
```

Expected:

- `cargo test -p montage-auth` passes.
- `rg` returns no active Montage-auth fallback or version-spoof references. Historical test fixtures inside unrelated upstream vendored tests may remain only if they are not used by Montage public defaults; justify any retained match in the commit message.

- [ ] **Step 6: Commit Task 1**

Run:

```bash
git add crates/auth vendor/codex-rs
git commit -m "fix: remove unsanctioned codex auth defaults"
```

- [ ] **Task 1 verification gate**

Run:

```bash
cargo test -p montage-auth
rg -n "app_EMoamEEZ73f0CkXaXp7hrann|0\\.128\\.0|requires a newer version|installed Codex 0\\.128\\.0|first-party client id" \
  crates/auth vendor/codex-rs/SOURCE vendor/codex-rs/login/src/auth vendor/codex-rs/models-manager/src vendor/codex-rs/exec/src vendor/codex-rs/analytics/src vendor/codex-rs/model-provider-info/src vendor/codex-rs/codex-api/src || true
git status --short
```

Expected:

- `cargo test -p montage-auth` passes.
- Remaining `rg` matches are either zero or explicitly documented as upstream-only fixtures not used by Montage public defaults.
- `git status --short` is empty.

---

### Task 2: Remove Social-Server Secret-Like Defaults And Project Identifiers

**Files:**
- Modify: `crates/social-server/run-local.sh`
- Modify: `docs/social-server/README.md`
- Test: shell syntax and text scans

- [ ] **Step 1: Write the failing scan**

Run:

```bash
rg -n "local-dev-token|local-internal-token|c2e3e7c14a025d829f0411ce456f1ca01b3f546218bf21892b3dd7cc0d9efedd|vgkocfbtkzmpklruqmsx|technologia-builder-network" \
  crates/social-server/run-local.sh docs/social-server/README.md
```

Expected: FAIL for readiness because matches are present.

- [ ] **Step 2: Update local script to fail fast**

Change `run-local.sh` so `DESKTOP_AUTH_TOKEN`, `SERVICE_SHARED_SECRET`, and `SOCIAL_TOKEN_AEAD_KEY` must be supplied by `.env.local` or the environment. Keep `SOCIAL_TOKEN_KEY_ID` defaulting to `local-k1` only if the encryption key is explicitly supplied.

The script must exit with a clear error for each missing value:

```text
DESKTOP_AUTH_TOKEN is unset. Put a random local value in crates/social-server/.env.local.
SERVICE_SHARED_SECRET is unset. Put a random local value in crates/social-server/.env.local.
SOCIAL_TOKEN_AEAD_KEY is unset. Generate one with: openssl rand -hex 32
```

- [ ] **Step 3: Generalize Supabase docs**

Replace the named Supabase project/ref in `docs/social-server/README.md` with placeholders:

```text
For your Supabase project:
- SUPABASE_URL=https://<project-ref>.supabase.co
- Schema montage_social + required tables + pg_cron/pg_net must be applied.
```

- [ ] **Step 4: Verify shell syntax and scans**

Run:

```bash
bash -n crates/social-server/run-local.sh
rg -n "local-dev-token|local-internal-token|c2e3e7c14a025d829f0411ce456f1ca01b3f546218bf21892b3dd7cc0d9efedd|vgkocfbtkzmpklruqmsx|technologia-builder-network" \
  crates/social-server/run-local.sh docs/social-server/README.md || true
```

Expected:

- `bash -n` passes.
- `rg` returns no matches.

- [ ] **Step 5: Commit Task 2**

Run:

```bash
git add crates/social-server/run-local.sh docs/social-server/README.md
git commit -m "chore: remove public social-server local secrets"
```

- [ ] **Task 2 verification gate**

Run:

```bash
bash -n crates/social-server/run-local.sh
rg -n "local-dev-token|local-internal-token|c2e3e7c14a025d829f0411ce456f1ca01b3f546218bf21892b3dd7cc0d9efedd|vgkocfbtkzmpklruqmsx|technologia-builder-network" \
  crates/social-server/run-local.sh docs/social-server/README.md || true
git status --short
```

Expected: shell syntax passes, scan returns no matches, worktree is clean.

---

### Task 3: Add Public Privacy And Data-Egress Disclosure

**Files:**
- Create: `PRIVACY.md`
- Modify: `README.md`
- Modify: `apps/desktop/src/app/auth/AuthChooser.tsx`
- Modify: `apps/desktop/src/app/WelcomeCard.tsx`

- [ ] **Step 1: Create privacy policy**

Create `PRIVACY.md` with these sections:

- Local-first scope.
- Files and project data stored locally.
- Data sent to OpenAI/ChatGPT or OpenAI API during agent use.
- Data sent to Anthropic when Anthropic-backed commands/indexers are used.
- Data sent to OpenRouter for generated media.
- Data sent to Deepgram when `DEEPGRAM_API_KEY` is configured.
- Data sent to Hugging Face/pyannote model infrastructure during gated model setup or downloads.
- Data sent to YouTube/social providers when publishing is configured.
- Secrets storage expectations.
- Contact path via `SECURITY.md` until a dedicated support channel exists.

- [ ] **Step 2: Link privacy policy from README**

Add a concise `Privacy and data egress` section to `README.md` linking `PRIVACY.md` and explaining that developer-preview users should review external provider behavior before importing sensitive media.

- [ ] **Step 3: Link privacy policy from auth/welcome UI**

Add one short visible sentence plus link in the auth/welcome surfaces:

```text
Review the privacy policy before connecting accounts or sending media-derived context to model providers.
```

- [ ] **Step 4: Verify docs and frontend build**

Run:

```bash
test -f PRIVACY.md
rg -n "OpenAI|Anthropic|OpenRouter|Deepgram|Hugging Face|YouTube|local" PRIVACY.md
rg -n "PRIVACY.md|privacy policy|Privacy" README.md apps/desktop/src/app/auth/AuthChooser.tsx apps/desktop/src/app/WelcomeCard.tsx
pnpm --dir apps/desktop build
```

Expected:

- All scans return matches.
- Desktop frontend build passes.

- [ ] **Step 5: Commit Task 3**

Run:

```bash
git add PRIVACY.md README.md apps/desktop/src/app/auth/AuthChooser.tsx apps/desktop/src/app/WelcomeCard.tsx
git commit -m "docs: add privacy and data egress disclosure"
```

- [ ] **Task 3 verification gate**

Run:

```bash
test -f PRIVACY.md
rg -n "OpenAI|Anthropic|OpenRouter|Deepgram|Hugging Face|YouTube|local" PRIVACY.md
pnpm --dir apps/desktop build
git status --short
```

Expected: privacy policy exists, required provider disclosures are present, frontend build passes, worktree is clean.

---

### Task 4: Expand Third-Party Notices And Asset Provenance

**Files:**
- Modify: `THIRD_PARTY_NOTICES.md`
- Create or modify: `apps/desktop/src/shell/assets/README.md`
- Optional create: `docs/legal/asset-provenance.md`

- [ ] **Step 1: Document demo shell assets**

Create `apps/desktop/src/shell/assets/README.md` explaining the source/provenance of every JPEG in that directory. If provenance cannot be proven, state that the asset must be replaced before a consumer release and must not be marketed as redistributable sample footage.

- [ ] **Step 2: Expand third-party notices**

Add concise notices for:

- OpenSSL / `openssl-sys`.
- pyannote speaker diarization model and CC-BY-4.0 attribution expectations.
- Deepgram as an optional external transcription/diarization processor.
- OpenRouter as an optional generated-media processor.
- shell demo JPEG assets with link to `apps/desktop/src/shell/assets/README.md`.
- bubblewrap LGPL source availability/relinking note, keeping the existing license path.

- [ ] **Step 3: Verify notices cover named surfaces**

Run:

```bash
rg -n "OpenSSL|openssl-sys|pyannote|CC-BY-4\\.0|Deepgram|OpenRouter|bubblewrap|LGPL|shell/assets|podcast" THIRD_PARTY_NOTICES.md apps/desktop/src/shell/assets/README.md
```

Expected: every named surface returns at least one match.

- [ ] **Step 4: Commit Task 4**

Run:

```bash
git add THIRD_PARTY_NOTICES.md apps/desktop/src/shell/assets/README.md
git commit -m "docs: expand third-party notices"
```

- [ ] **Task 4 verification gate**

Run:

```bash
rg -n "OpenSSL|openssl-sys|pyannote|CC-BY-4\\.0|Deepgram|OpenRouter|bubblewrap|LGPL|shell/assets|podcast" THIRD_PARTY_NOTICES.md apps/desktop/src/shell/assets/README.md
git status --short
```

Expected: all notice scans return matches, worktree is clean.

---

### Task 5: Tighten Git Hygiene For Public Contributions

**Files:**
- Modify: `.gitignore`
- Create: `.gitattributes`
- Modify: `.github/PULL_REQUEST_TEMPLATE.md`

- [ ] **Step 1: Add ignore rules**

Add these rules to `.gitignore` near related sections:

```gitignore
*.pptx
*.docx
*.xlsx
*.pyc
```

- [ ] **Step 2: Add binary attributes**

Create `.gitattributes` with:

```gitattributes
*.png binary
*.jpg binary
*.jpeg binary
*.gif binary
*.webp binary
*.wav binary
*.mp3 binary
*.mp4 binary
*.mov binary
*.pdf binary
```

- [ ] **Step 3: Add DCO checkbox to PR template**

Add this checkbox to `.github/PULL_REQUEST_TEMPLATE.md`:

```markdown
- [ ] I certify that I have the right to submit this work under the repository license and can add a `Signed-off-by` line if maintainers request DCO confirmation.
```

- [ ] **Step 4: Verify hygiene rules**

Run:

```bash
git check-ignore -v "example.pptx" "example.docx" "example.xlsx" "example.pyc"
test -f .gitattributes
rg -n "\\*\\.png binary|\\*\\.mp4 binary|\\*\\.pdf binary" .gitattributes
rg -n "Signed-off-by|DCO|right to submit" .github/PULL_REQUEST_TEMPLATE.md
```

Expected: ignore checks identify `.gitignore`; attributes and PR template scans pass.

- [ ] **Step 5: Commit Task 5**

Run:

```bash
git add .gitignore .gitattributes .github/PULL_REQUEST_TEMPLATE.md
git commit -m "chore: tighten public contribution hygiene"
```

- [ ] **Task 5 verification gate**

Run:

```bash
git check-ignore -v "example.pptx" "example.docx" "example.xlsx" "example.pyc"
rg -n "\\*\\.png binary|\\*\\.mp4 binary|\\*\\.pdf binary" .gitattributes
rg -n "Signed-off-by|DCO|right to submit" .github/PULL_REQUEST_TEMPLATE.md
git status --short
```

Expected: all checks pass, worktree is clean.

---

### Task 6: Add CI License, Advisory, And Secret-Scanning Gates

**Files:**
- Create: `deny.toml`
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add cargo-deny policy**

Create root `deny.toml` with an Apache-friendly allowlist and explicit handling for known vendored/copyleft surfaces. Start minimal:

```toml
[licenses]
allow = [
  "Apache-2.0",
  "MIT",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Unicode-3.0",
  "CC0-1.0",
]
confidence-threshold = 0.8

[advisories]
ignore = []
```

- [ ] **Step 2: Add CI jobs**

Update `.github/workflows/ci.yml` with a separate `security` job that:

- checks out the repo;
- installs stable Rust;
- installs `cargo-deny`;
- runs `cargo deny check`;
- runs TruffleHog or Gitleaks against the checked-out tree.

- [ ] **Step 3: Run local policy verification**

Run:

```bash
cargo install cargo-deny --locked || true
cargo deny check
```

If `cargo deny check` fails because the initial policy is too strict for existing dependencies, update `deny.toml` with specific, documented exceptions only. Do not use broad wildcard exceptions.

- [ ] **Step 4: Verify workflow contains both gates**

Run:

```bash
rg -n "cargo deny check|cargo-deny|gitleaks|trufflehog|secret" .github/workflows/ci.yml deny.toml
```

Expected: CI workflow contains dependency/advisory and secret-scanning gates.

- [ ] **Step 5: Commit Task 6**

Run:

```bash
git add deny.toml .github/workflows/ci.yml
git commit -m "ci: add license advisory and secret scanning"
```

- [ ] **Task 6 verification gate**

Run:

```bash
cargo deny check
rg -n "cargo deny check|cargo-deny|gitleaks|trufflehog|secret" .github/workflows/ci.yml deny.toml
git status --short
```

Expected: cargo-deny passes, workflow scans pass, worktree is clean.

---

### Task 7: Clarify Developer-Preview README And Community Entry Points

**Files:**
- Modify: `README.md`
- Modify: `CONTRIBUTING.md`
- Modify: `MAINTAINERS.md`
- Create: `CHANGELOG.md`
- Create: `ARCHITECTURE.md`

- [ ] **Step 1: Update README positioning**

Make README explicitly say:

- this is a developer-preview source release;
- consumer installers are not ready yet;
- release packaging/signing/notarization are deferred to a consumer-release track;
- how contributors can run the current development checks;
- links to `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, `PRIVACY.md`, `THIRD_PARTY_NOTICES.md`, `ARCHITECTURE.md`, and `CHANGELOG.md`.

- [ ] **Step 2: Add contributor starting points**

Add a short `Where to start` section to `CONTRIBUTING.md` that points new contributors to:

- docs and architecture;
- narrow issues labeled `good first issue`;
- required checks before opening a PR.

- [ ] **Step 3: Add maintainer contact**

Add a public maintainer contact placeholder or GitHub Issues/Discussions route to `MAINTAINERS.md`. Do not add private personal contact data unless the user explicitly supplies it.

- [ ] **Step 4: Add changelog**

Create `CHANGELOG.md` with:

```markdown
# Changelog

## Unreleased

- Prepared the repository for developer-preview open-source publication.
- Consumer installers, signing, notarization, and auto-update support remain future release work.
```

- [ ] **Step 5: Add architecture guide**

Create `ARCHITECTURE.md` with one paragraph each for:

- `crates/`
- `apps/desktop/`
- `python/`
- `skills/`
- `vendor/codex-rs/`
- social publishing crates/server

Include a short ASCII data-flow diagram from media import to indexers to agent tools to render/export.

- [ ] **Step 6: Verify docs are linked**

Run:

```bash
test -f CHANGELOG.md
test -f ARCHITECTURE.md
rg -n "developer-preview|consumer installers|CONTRIBUTING.md|CODE_OF_CONDUCT.md|SECURITY.md|PRIVACY.md|THIRD_PARTY_NOTICES.md|ARCHITECTURE.md|CHANGELOG.md" README.md
rg -n "good first issue|Where to start|cargo fmt|cargo clippy|cargo check" CONTRIBUTING.md
rg -n "Issues|Discussions|contact|maintainer" MAINTAINERS.md
```

Expected: all scans return matches.

- [ ] **Step 7: Commit Task 7**

Run:

```bash
git add README.md CONTRIBUTING.md MAINTAINERS.md CHANGELOG.md ARCHITECTURE.md
git commit -m "docs: clarify developer preview open-source launch"
```

- [ ] **Task 7 verification gate**

Run:

```bash
test -f CHANGELOG.md
test -f ARCHITECTURE.md
rg -n "developer-preview|consumer installers|CONTRIBUTING.md|CODE_OF_CONDUCT.md|SECURITY.md|PRIVACY.md|THIRD_PARTY_NOTICES.md|ARCHITECTURE.md|CHANGELOG.md" README.md
rg -n "good first issue|Where to start|cargo fmt|cargo clippy|cargo check" CONTRIBUTING.md
rg -n "Issues|Discussions|contact|maintainer" MAINTAINERS.md
git status --short
```

Expected: docs exist, links are present, worktree is clean.

---

### Task 8: Final Open-Source Readiness Audit

**Files:**
- Create: `docs/superpowers/plans/2026-06-08-open-source-readiness-final.md`

- [ ] **Step 1: Run final scans**

Run:

```bash
{
  echo "# Open-Source Readiness Final Verification"
  echo
  echo "Date: 2026-06-08"
  echo "Branch: $(git branch --show-current)"
  echo
  echo "## Git"
  echo '```text'
  git status --short
  git log --oneline -8
  echo '```'
  echo
  echo "## Forbidden Public Defaults Scan"
  echo '```text'
  rg -n "local-dev-token|local-internal-token|c2e3e7c14a025d829f0411ce456f1ca01b3f546218bf21892b3dd7cc0d9efedd|vgkocfbtkzmpklruqmsx|technologia-builder-network|app_EMoamEEZ73f0CkXaXp7hrann|installed Codex 0\\.128\\.0|requires a newer version" \
    crates/auth vendor/codex-rs/SOURCE vendor/codex-rs/login/src/auth vendor/codex-rs/models-manager/src vendor/codex-rs/exec/src vendor/codex-rs/analytics/src vendor/codex-rs/model-provider-info/src vendor/codex-rs/codex-api/src crates/social-server docs/social-server || true
  echo '```'
  echo
  echo "## Required Files"
  echo '```text'
  ls -1 PRIVACY.md THIRD_PARTY_NOTICES.md CHANGELOG.md ARCHITECTURE.md deny.toml .gitattributes
  echo '```'
} > docs/superpowers/plans/2026-06-08-open-source-readiness-final.md
```

- [ ] **Step 2: Run final checks**

Run:

```bash
cargo fmt --all -- --check
cargo test -p montage-auth
pnpm --dir apps/desktop build
cargo deny check
bash -n crates/social-server/run-local.sh
```

- [ ] **Step 3: Review final receipt**

Open `docs/superpowers/plans/2026-06-08-open-source-readiness-final.md` and confirm:

- git status section is empty before the final receipt is committed;
- forbidden public defaults scan has no active blockers, or retained matches are documented as upstream-only fixtures;
- required files are listed.

- [ ] **Step 4: Commit final audit**

Run:

```bash
git add docs/superpowers/plans/2026-06-08-open-source-readiness-final.md
git commit -m "docs: record open-source readiness verification"
```

- [ ] **Task 8 verification gate**

Run:

```bash
cargo fmt --all -- --check
cargo test -p montage-auth
pnpm --dir apps/desktop build
cargo deny check
bash -n crates/social-server/run-local.sh
git status --short
git log --oneline -8
```

Expected:

- all commands pass;
- `git status --short` is empty;
- the last eight commits correspond to Tasks 1-8 plus the plan/baseline commit;
- source-publication blockers from the audit are resolved or explicitly deferred as consumer-release work.

---

## Explicitly Deferred To Consumer-Release Track

These are important but are not part of this open-source source-publication goal:

- macOS Developer ID signing and notarization.
- Windows Authenticode signing.
- GitHub release workflow for downloadable installers.
- Auto-updater.
- Bundled ffmpeg/ffprobe sidecars.
- Bundled Python/uv/model stack.
- First-launch consumer auth gate.
- Full end-user documentation and installer troubleshooting.
