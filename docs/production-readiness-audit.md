# Montage — Production Readiness Audit

**Date:** 2026-06-07
**Auditor:** Automated multi-dimension review against verified findings

---

## 2026-06-10 Refresh Against `origin/main`

This document started as a June 7 audit. It is now a historical baseline plus
this status update. The current checked-out `main` is `a1a66d34` and matches
`origin/main`.

### Current Verdict

| Goal | Current status | What changed since June 7 |
|------|----------------|---------------------------|
| **G1 - Open-source launch** | **Closer, still needs legal/product review** | Community and hygiene files landed; security hygiene CI now runs `cargo deny` and gitleaks; hardcoded social-server local secrets were removed. Remaining concern is the vendored Codex compatibility/version behavior and whether the sanctioned OAuth/client story is approved. |
| **G2 - Production-ready** | **Not ready** | CI and provider-key handling improved, but runtime sandbox integration, crash reporting, updater, eval gate, token-budget enforcement, and some path confinement gaps remain. |
| **G3 - Downloadable consumer builds** | **Partial macOS path only** | A release workflow, sidecar declarations, ffmpeg/ffprobe resolution, `uv` sidecar, Python resource handling, and privacy policy now exist on `main`. Windows signing, Intel macOS release coverage, updater, full Python environment/model strategy, and a hard first-launch auth/consent gate remain incomplete. |

### Items Now Completed On `main`

- **Release workflow exists:** `.github/workflows/release.yml` builds a macOS DMG on `v*` tags or manual dispatch, checks required Apple secrets, imports the certificate, runs Tauri build, notarizes/staples, uploads artifacts, and publishes a GitHub Release.
- **Desktop sidecars are declared:** `apps/desktop/src-tauri/tauri.conf.json` now includes sidecars for `codex`, `ffmpeg`, `ffprobe`, `montage-mcp-server`, `uv`, and `yt-dlp`; `Makefile` has corresponding fetch/build targets.
- **ffmpeg/ffprobe are no longer PATH-only:** `crates/render/src/ffmpeg.rs` resolves env overrides, packaged sibling sidecars, then PATH/common install dirs.
- **Privacy policy exists:** `PRIVACY.md` is present and linked from `WelcomeCard` and `AuthChooser`.
- **Social-server local secret defaults were removed:** `crates/social-server/run-local.sh` now fails fast when required local env vars such as `SOCIAL_TOKEN_AEAD_KEY` are unset.
- **Security hygiene CI exists:** `deny.toml`, `.gitleaks.toml`, and CI steps for `cargo deny check` and gitleaks are present.
- **Provider-key settings exist:** desktop settings now cover `HF_TOKEN`, `DEEPGRAM_API_KEY`, `OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY`, `PEXELS_API_KEY`, and `X_BEARER_TOKEN`.
- **OSS hygiene improved:** `ARCHITECTURE.md`, `CHANGELOG.md`, `.gitattributes`, expanded `THIRD_PARTY_NOTICES.md`, and README privacy/release references are present.

### Still Blocking Or Incomplete On `main`

- **Windows release/signing:** no Windows Authenticode path or Windows release artifacts were found.
- **macOS release coverage mismatch:** the release workflow covers Apple Silicon DMG; README/release language should not imply full Intel macOS coverage unless x86_64 artifacts are actually built.
- **No updater path:** no `tauri-plugin-updater` or updater config is wired.
- **No production crash reporting:** no desktop `sentry::init()` or production panic/crash reporting hook is wired outside vendored TUI code.
- **Runtime sandbox still appears unwired:** no current non-vendor production path was found calling `montage_sandboxing::Sandbox::run`.
- **Eval gate remains broken:** `.github/workflows/evals.yml` still invokes `cargo run -p montage-eval`, but no current `montage-eval` crate is present.
- **Audit tooling is partial:** `cargo deny` exists, but `cargo audit` and Python/uv audit steps were not found.
- **First-launch auth and data consent are not hard gates:** privacy links exist, but current `main` still stores only `montage:welcome:shown`, lets Esc/close dismiss the welcome card, and does not record timestamped data-flow consent.
- **Legacy chat composer is not auth-gated:** the current `apps/desktop/src/agent/Composer.tsx` can still submit `start_turn` when a project is open and auth is missing. Several newer shell surfaces are gated, but this older agent path remains a consumer-readiness gap.
- **OpenRouter cost confirmation is incomplete:** provider keys and privacy disclosure exist, but current `main` does not add an OpenRouter cost-confirmation approval key, does not read `OPENROUTER_VIDEO_COST_ESTIMATE_USD`, and does not display generated-media cost labels.
- **Python packaging is partial:** `uv` and the `python` resource are bundled, but the prebuilt environment/model-weight/first-launch install strategy is not complete.
- **Path confinement gaps remain likely:** thumbnail and Python MCP asset-path confinement still need focused fixes or proof.
- **TikTok/Instagram remain present despite stubbed paths:** keep them hidden/coming-soon until OAuth/upload implementations are real.
- **Token budget/usage enforcement remains unwired:** vendored protocol support exists, but no product-level default budget gate was verified.

### Consumer Release Branch Compared To Current `main`

Update: `codex/consumer-readiness-salvage` now manually ports the useful pieces
from this stale branch onto current `main`. Until that PR is merged, the
`origin/main` blockers below remain true for main, but the salvage source no
longer needs to be kept for these specific items.

2026-06-10 MVP user-access update on the same branch:

- **Provider setup guidance:** desktop Provider Keys settings now link users
  directly to provider key pages, and in-app help/privacy links point to the
  public Montage setup/privacy pages instead of GitHub source docs.
- **User-facing public surface:** the Portfolio site has `/montage`,
  `/montage/setup`, `/montage/privacy`, and `/montage/terms`, with `/awidat/*`
  redirects kept for old links.
- **Simple support visibility:** Settings > About exposes an Open logs action
  so users can send logs for crashes, failed indexers, failed provider calls, or
  export failures without knowing filesystem paths.
- **Stub social surfaces hidden for MVP:** TikTok and Instagram remain in
  internal compatibility paths, but default desktop publishing, delivery, upload
  preferences, and scheduler provider lists hide them until those paths are real
  enough for invited users.
- **Python/uv proof:** `uv` remains a required sidecar, the Tauri app bundles
  the `python` resource, release CI fetches/verifies `uv`, and focused
  `montage-config` tests pass for bundled `uv`, bundled Python resource layout,
  and `UV_PROJECT_ENVIRONMENT` resolution. Local full sidecar verification still
  requires fetching/building all release sidecars; this dev checkout only had
  `yt-dlp` before `make desktop-uv TARGET_TRIPLE=aarch64-apple-darwin` fetched
  the current-target `uv` binary.

`codex/consumer-release-readiness` is a stale local worktree at `38e6a5c`. It is
**24 commits ahead** of `origin/main` and **161 commits behind**. `git cherry -v
origin/main HEAD` shows all 24 commits as non-patch-equivalent to main, but a
two-dot diff from current main to the branch is destructive because the branch
predates many merged fixes.

Do **not** merge or cherry-pick the branch wholesale. `git merge-tree --messages
origin/main HEAD` reports conflicts across release workflow, `CHANGELOG.md`,
`PRIVACY.md`, `Makefile`, README, Tauri config, desktop auth/settings/shell
components, config defaults, generated media, b-roll search, render ffmpeg, and
binary app icons.

#### Useful Work To Salvage Manually

- **First-launch data consent:** **ported in `codex/consumer-readiness-salvage`.**
  Branch changes `useWelcome` from
  `montage:welcome:shown` to `montage:welcome:consent`, records `consentedAt`,
  and changes the welcome CTA to explicit "I understand". Port this into the
  current glass/BrandMark welcome design instead of taking the branch component.
- **Legacy agent auth gate:** **ported in `codex/consumer-readiness-salvage`.**
  Branch gates the legacy composer and some shell
  command paths with `isAuthReadyForAgent`, disables unauthenticated sends, and
  opens auth from "Sign in to get started". Port the behavior into current main's
  newer component structure.
- **OpenRouter cost controls:** **ported in `codex/consumer-readiness-salvage`.**
  Branch adds cost-confirmation text to the
  generated-media approval key, supports `OPENROUTER_VIDEO_COST_ESTIMATE_USD`,
  records actual provider cost fields when present, and renders cost labels in
  `GeneratedMediaPanel`. Port after reconciling with main's provider-key vault
  and generated-media UI.
- **Consumer proof docs:** **refreshed in `codex/consumer-readiness-salvage`.**
  Branch docs were treated as historical receipts only; new docs avoid claiming
  an old debug bundle as current proof.

#### Branch Work Superseded By `main`

- **Release workflow and signing skeleton:** current `main` has the newer
  `.github/workflows/release.yml`; the branch workflow is older and conflicts.
- **Tauri sidecars/resources:** current `main` declares `codex`, `ffmpeg`,
  `ffprobe`, `montage-mcp-server`, `uv`, and `yt-dlp`, and includes the `python`
  resource. The branch config is missing `codex` and has older CSP/resource
  shape.
- **ffmpeg/ffprobe resolution:** current `main` has the PR86 sidecar/env/PATH
  resolver; branch render code is older.
- **Python/uv resource resolution:** current `main` has the stronger
  `UV_PROJECT_ENVIRONMENT` and bundled-python path handling. The branch proves
  intent, not current completeness.
- **Provider-key settings:** current `main` has the newer provider-key vault and
  settings surface for Pexels, OpenRouter, Anthropic, Hugging Face, Deepgram, and
  publishing keys. Do not port the older branch settings implementation.

### Worktrees With Work Not In `origin/main`

| Worktree | Branch | State | Importance |
|----------|--------|-------|------------|
| None | - | - | The stale `codex/consumer-release-readiness` worktree and local-only branch were removed after the salvage PR was opened. |

All other worktrees from this refresh were clean and removed. The only
registered worktree is the main checkout, currently on the fresh
`codex/consumer-readiness-salvage` PR branch.

Additional non-worktree branch to note: `codex/awidat-editor-harness-improvements`
is pushed but very stale and pre-current-rename shaped (`awidat-eval`, not a
current `montage-eval`). Do not use it as evidence that the eval gate is fixed.

### Audit Corrections From The June 7 Text Below

- Treat June 7 claims of "no release workflow", "ffmpeg not bundled",
  "montage-mcp-server not declared", "no privacy policy", "no security hygiene
  CI", and "hardcoded social-server AEAD default" as stale.
- Treat June 7 claims around Windows signing, updater, crash reporting, eval
  gate, runtime sandboxing, consent gate, auth hard-gate, and token-budget
  enforcement as still relevant.
- Treat Python/uv bundling as **partial**, not absent and not complete.
- Treat provider-key GUI/setup as **mostly complete on main**, while OpenRouter
  cost confirmation/display remains incomplete.
- Treat Codex OAuth/version behavior as **changed but still review-sensitive**:
  `MONTAGE_OAUTH_CLIENT_ID` is now required for sanctioned ChatGPT OAuth paths,
  but vendored Codex compatibility references to `0.128.0` still exist and need
  legal/product review before a public launch.

The original June 7 findings and roadmap below are intentionally preserved for
historical context. Use this refresh section as the current source of truth.

---

## Executive Summary

Montage is a technically ambitious, agent-native video editing harness with a well-structured Rust/Tauri/React codebase, solid lint enforcement, and a functioning ChatGPT OAuth flow. However, the project carries fourteen blocker-severity gaps that collectively prevent all three release goals from being achieved in its current state. The most critical cluster: no release CI workflow exists, no code signing or notarization is configured, ffmpeg and the Python indexer stack are unbundled, and the vendored OpenAI codex fork deliberately spoofs a competitor client version to bypass model-access gates — a legal and ToS exposure that affects the open-source launch and every downloadable binary. Privacy and consent infrastructure is entirely absent (no privacy policy, no data-flow disclosures, no consent gate). The project is well-positioned for a developer-preview open-source release once the legal/ToS issues are resolved and the README is updated, but G2 production-readiness and G3 consumer-installer goals require a dedicated packaging, signing, and compliance phase before any public announcement.

---

## GO / NO-GO Verdict

| Goal | Status | Justification |
|------|--------|---------------|
| **G1 — Open-source launch** | **Close** | Community files and CI exist; blocked by version-spoofing ToS exposure in vendored fork, hardcoded AEAD key in run-local.sh, Supabase project ref in history, no privacy policy, and README that contradicts the project's GUI goal |
| **G2 — Production-ready** | **Not ready** | Sandbox is unconnected at runtime, no crash reporting, no auto-updater, no audit/deny CI step, no token budget enforcement, no privacy/consent infrastructure, 21k+ lines of Tauri command handlers are untested |
| **G3 — Downloadable consumer builds** | **Not ready** | Zero release CI workflow, no macOS notarization or Windows signing, ffmpeg not bundled, Python/uv stack not bundled, montage-mcp-server not declared as sidecar, no auth gate on first launch, no privacy policy, no consent gate |

---

## Top Blockers

Items that prevent at least one goal from shipping at all, ordered by breadth of impact:

1. **Version spoofing in vendored codex fork** — `vendor/codex-rs/login/src/auth/default_client.rs` stamps `build_version = "0.128.0"` to impersonate the official Codex CLI. Visible in a public repo; exposes the project to OpenAI ToS enforcement and takedowns. Affects G1, G2, G3.
2. **No release CI workflow / no signed installer path** — `.github/workflows/` contains only `ci.yml` and `evals.yml`; neither runs `tauri build`, produces artifacts, or signs anything. README and CONTRIBUTING explicitly state packaging is broken. Affects G3 entirely.
3. **No macOS notarization or Windows Authenticode signing** — `tauri.conf.json` has no signing block; zero cert/signing references exist in the repo. Gatekeeper and SmartScreen will block unsigned binaries for all non-technical users. Affects G3.
4. **ffmpeg/ffprobe not bundled** — every render, proxy, thumbnail, and waveform call fails for users without system ffmpeg; error message tells them to use a package manager. `crates/render/src/ffmpeg.rs:6-7` explicitly defers bundling to "v2". Affects G3.
5. **Python/uv stack not bundled** — all 12 indexers (transcription, scene detect, face detect, beats, etc.) require system `uv` and `python`, plus ~3 GB of model weights. Non-technical users have no path to enable them. `crates/config/src/defaults.rs:208-226`. Affects G3.
6. **No privacy policy and no data-flow consent gate** — no PRIVACY.md, no in-app disclosure that transcripts go to OpenAI, no consent before processing. GDPR Art. 13/14, CCPA, and App Store review all require this. Affects G1, G2, G3.
7. **No auth gate on first launch** — user can import media, type a prompt, and fire `start_turn` with no credential; the failure surfaces as a raw error string toast. `apps/desktop/src/shell/empty/Landing.tsx` has no sign-in affordance. Affects G3.
8. **Cryptographic AEAD key hardcoded in committed script** — `crates/social-server/run-local.sh:53` contains a default 64-hex ChaCha20-Poly1305 key that will be public post G1 launch, permanently compromising any deployment that used the default. Affects G1, G2.
9. **montage-mcp-server not declared as Tauri sidecar** — `tauri.conf.json` `externalBin` lists only `yt-dlp`; the agent falls back to shell-only mode in any packaged build. Error message tells users to run `cargo build`. `apps/desktop/src-tauri/src/codex_session.rs:97-100`. Affects G3, G2.
10. **montage-eval crate directory is empty** — `crates/eval/` has zero files; `evals.yml` calls `cargo run -p montage-eval` which fails with "package not found". The entire eval gate is inert. Affects G2, G3.

---

## Prioritized Remediation Roadmap

### P0 — Blockers (must fix before any public launch)

| # | Action | Files |
|---|--------|-------|
| P0-1 | Contact OpenAI to obtain a sanctioned client ID or register a first-party Montage OAuth app; remove the `build_version = "0.128.0"` spoof or add a legal disclaimer | `vendor/codex-rs/login/src/auth/default_client.rs`, `vendor/codex-rs/SOURCE` |
| P0-2 | Remove the hardcoded AEAD default from `run-local.sh`; add an `exit 1` guard when `SOCIAL_TOKEN_AEAD_KEY` is unset; rotate the key in any live deployment | `crates/social-server/run-local.sh:53` |
| P0-3 | Scrub the Supabase project ref `vgkocfbtkzmpklruqmsx` from `run-local.sh` and `docs/social-server/README.md` using `git filter-repo` or BFG before the public push | `crates/social-server/run-local.sh:76`, `docs/social-server/README.md:124-125` |
| P0-4 | Create `.github/workflows/release.yml` triggered on `v*` tags; run `cargo tauri build` per platform, upload signed artifacts, publish GitHub Release | `.github/workflows/release.yml` (new) |
| P0-5 | Configure macOS code signing and notarization: add `bundle.macOS.signingIdentity`, `bundle.macOS.entitlements` to `tauri.conf.json`; store `APPLE_CERTIFICATE`, `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_PASSWORD` as GitHub secrets | `apps/desktop/src-tauri/tauri.conf.json`, `.github/workflows/release.yml` |
| P0-6 | Configure Windows Authenticode signing: add EV certificate to release workflow; store `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` as GitHub secrets | `.github/workflows/release.yml`, `apps/desktop/src-tauri/tauri.conf.json` |
| P0-7 | Bundle static ffmpeg+ffprobe as Tauri `externalBin` sidecars using the same pattern as `yt-dlp`; update `crates/render/src/ffmpeg.rs` to resolve the sidecar path; update `apps/desktop/src/media/readiness.ts` error message | `apps/desktop/src-tauri/tauri.conf.json`, `crates/render/src/ffmpeg.rs`, `apps/desktop/src/media/readiness.ts`, `Makefile` |
| P0-8 | Add `montage-mcp-server` to `tauri.conf.json` `externalBin` and to the release pre-build step; change the missing-binary error message to "reinstall the app" | `apps/desktop/src-tauri/tauri.conf.json`, `apps/desktop/src-tauri/src/codex_session.rs:97-100` |
| P0-9 | Add first-launch auth gate: call `auth_status` on app start; if `mode === 'none'`, open `AuthChooser` before the `Landing` screen or block the agent input with "Sign in to get started" | `apps/desktop/src/App.tsx`, `apps/desktop/src/state/auth.ts`, `apps/desktop/src/shell/empty/Landing.tsx` |
| P0-10 | Create `PRIVACY.md` (or hosted privacy policy page) enumerating all data egress; link from `AuthChooser` and `WelcomeCard`; add a one-sentence disclosure before first auth | `apps/desktop/src/app/auth/AuthChooser.tsx`, `apps/desktop/src/app/WelcomeCard.tsx`, `PRIVACY.md` (new) |
| P0-11 | Add data-flow consent gate to `WelcomeCard`: replace single "Get started" button with two-step flow (data summary + "I understand"); record timestamped consent to localStorage | `apps/desktop/src/app/WelcomeCard.tsx`, `apps/desktop/src/state/welcome.ts` |
| P0-12 | Decide on Python/uv bundling strategy: either ship a vendored `uv` binary + pre-built `.venv` tarball in Tauri `resources/`, or move high-value indexers (transcription, scene detect) to a hosted API; add a visible first-launch progress dialog | `apps/desktop/src-tauri/tauri.conf.json`, `crates/config/src/defaults.rs`, `apps/desktop/src-tauri/src/` |
| P0-13 | Scaffold `crates/eval/` with at minimum a `main.rs` that handles `--ci`, `--product`, `--golden`, `--stress`, `--live` flags; add it to `Cargo.toml` workspace members | `crates/eval/` (new), `Cargo.toml` |

### P1 — High priority (required for G2/G3 quality bar)

| # | Action | Files |
|---|--------|-------|
| P1-1 | Add `tauri-plugin-updater` to `Cargo.toml`, register it in `lib.rs`, configure `plugins.updater.endpoints` and `pubkey` in `tauri.conf.json`; generate signing keypair | `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/src-tauri/src/lib.rs`, `apps/desktop/src-tauri/tauri.conf.json` |
| P1-2 | Add `tauri-plugin-log` (or `tracing-subscriber` with file appender) to Tauri backend; call `.plugin(tauri_plugin_log::Builder::new().build())` in `lib.rs` | `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/src-tauri/src/lib.rs` |
| P1-3 | Wire `sentry::init()` or a `std::panic::set_hook` crash reporter in the Tauri entrypoint (with opt-in); remove unused `sentry = "0.46.0"` workspace dep if not wiring | `apps/desktop/src-tauri/src/lib.rs`, `Cargo.toml` |
| P1-4 | Connect `montage_sandboxing::Sandbox::run()` into render job spawn path in `crates/core/src/tools/start_render.rs`; or document explicitly that the codex approval gate is the primary control and retire the dead `SandboxFirst` machinery | `crates/core/src/tools/start_render.rs`, `crates/core/src/tool.rs` |
| P1-5 | Add `deny.toml` at repo root with SPDX allowlist; add `cargo deny check` and `cargo audit` steps to `ci.yml`; add `uv pip audit` for Python workspace | `.github/workflows/ci.yml`, `deny.toml` (new) |
| P1-6 | Add `windows-latest` to the `rust` job matrix and `desktop` job in `ci.yml`; add `tauri build --debug` step on at least `macos-latest` | `.github/workflows/ci.yml` |
| P1-7 | Detect token refresh failures in the turn-end event handler; auto-open `AuthChooser` on session expiry instead of showing a raw error toast | `apps/desktop/src/state/auth.ts`, `apps/desktop/src-tauri/src/codex_session.rs`, `apps/desktop/src/App.tsx` |
| P1-8 | Add `PEXELS_API_KEY` to `RESOLVE_AT_STARTUP` in `secrets.rs`; add a Pexels key row to `SettingsModal`; fix the error message in both `search_broll.rs` files to reference `montage secrets-set pexels_api_key` | `apps/desktop/src-tauri/src/secrets.rs:46-50`, `apps/desktop/src/app/SettingsModal.tsx`, `crates/core/src/tools/search_broll.rs:179`, `crates/core/src/montage_mcp/tools/search_broll.rs:122` |
| P1-9 | Add OpenRouter key row to `SettingsModal` with link to openrouter.ai and cost explanation; wire a Tauri command to call `montage_secrets::set` | `apps/desktop/src/app/SettingsModal.tsx`, `apps/desktop/src-tauri/src/commands/` |
| P1-10 | Surface pre-submission cost warning for OpenRouter video generation; query `/models` endpoint to populate `cost_estimate_usd`; render cost in `GeneratedMediaPanel` | `crates/core/src/generated_media/openrouter.rs`, `crates/core/src/tools/start_generated_media_job.rs`, `apps/desktop/src/media/GeneratedMediaPanel.tsx` |
| P1-11 | Add in-app disclosure that Deepgram receives raw audio when `DEEPGRAM_API_KEY` is present; provide a UI switch to local-only Whisper mode | `python/packages/whisper-mcp/src/whisper_mcp/__init__.py`, `apps/desktop/src/` |
| P1-12 | Change `AuthCredentialsStoreMode` default to `Auto` (keychain-first) for the Tauri desktop app launch path | `crates/auth/src/env.rs`, `apps/desktop/src-tauri/src/codex_session.rs` |
| P1-13 | Add path-traversal confinement to `generate_thumbnails_for_asset` and `list_thumbnail_frames` Tauri commands | `apps/desktop/src-tauri/src/commands/thumbnail.rs:174, 132` |
| P1-14 | Set a default per-session token budget via `ThreadGoalSetParams` in the codex bridge launch path; expose it as a user setting once `Settings` screen is live | `crates/codex-bridge/src/lib.rs`, `apps/desktop/src-tauri/src/codex_session.rs` |
| P1-15 | Implement a minimal `Settings` screen; re-enable `app:settings` menu item and wire `app:check_updates` to open GitHub Releases page | `apps/desktop/src-tauri/src/app_menu.rs`, `apps/desktop/src/app/menuCommands.ts` |
| P1-16 | Register a real bundle identifier (e.g. `com.awidat.montage`) in Apple's developer portal and update `tauri.conf.json:4` | `apps/desktop/src-tauri/tauri.conf.json` |
| P1-17 | Add a no-privacy-policy notice to `THIRD_PARTY_NOTICES.md` covering OpenRouter as a data processor; add Deepgram and pyannote model entries | `THIRD_PARTY_NOTICES.md` |
| P1-18 | Remove TikTok and Instagram from the provider registry (or mark "Coming Soon") until `complete_oauth` / `upload` stubs are replaced | `apps/desktop/src-tauri/src/publishing/tiktok.rs`, `apps/desktop/src-tauri/src/publishing/instagram.rs` |
| P1-19 | Add `sign-out` confirmation dialog before calling `auth_logout` | `apps/desktop/src/app/auth/AuthChooser.tsx:137-141` |
| P1-20 | Call `useAuth.getState().refresh()` once in `startupHydration.ts` so returning users see correct auth status from first render | `apps/desktop/src/` |
| P1-21 | Add `ANTHROPIC_API_KEY` and `HF_TOKEN` entry points to `SettingsModal` with setup guidance and links | `apps/desktop/src/app/SettingsModal.tsx` |

### P2 — Medium / low priority (production polish and G1 hygiene)

| # | Action | Files |
|---|--------|-------|
| P2-1 | Add `// SPDX-License-Identifier: Apache-2.0` and copyright line to all `.rs` files under `crates/` (scripted commit) | `crates/**/*.rs` |
| P2-2 | Add DCO requirement: add `Signed-off-by` checkbox to `.github/PULL_REQUEST_TEMPLATE.md`; configure DCO GitHub Action | `.github/PULL_REQUEST_TEMPLATE.md`, `.github/workflows/` |
| P2-3 | Add `gitleaks` or `trufflehog` secret-scanning step to CI | `.github/workflows/ci.yml` |
| P2-4 | Add `CHANGELOG.md` and adopt `v*` git tag discipline; wire `release.yml` trigger to version tags | `CHANGELOG.md` (new), `.github/workflows/release.yml` |
| P2-5 | Enable GitHub branch protection on `main` requiring `rust (macos-latest)`, `rust (ubuntu-latest)`, and `desktop` CI checks | GitHub repo settings |
| P2-6 | Add `ARCHITECTURE.md` with one-paragraph description of each major crate and an ASCII data-flow diagram | `ARCHITECTURE.md` (new) |
| P2-7 | Add "Contributing" section to `README.md` linking to `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md` | `README.md` |
| P2-8 | Create `docs/user-guide/` with Getting Started, indexing explainer, agent usage, and troubleshooting guides | `docs/user-guide/` (new) |
| P2-9 | Replace `eprintln!` in `crates/proto/src/project.rs:442-445` with `tracing::warn!` and a desktop event-channel notification | `crates/proto/src/project.rs` |
| P2-10 | Add `focus-trap-react` (or manual implementation) to all modals (`SettingsModal`, `ManageProjectsDialog`, `WelcomeCard`, `AgentsMdEditor`); add `role='dialog'` to `ManageProjectsDialog` | `apps/desktop/src/app/SettingsModal.tsx`, `apps/desktop/src/app/ManageProjectsDialog.tsx` |
| P2-11 | Fix `App.css:32` focus ring to opaque token: replace `box-shadow: var(--focus-ring)` with `outline: 2px solid var(--color-border-focus); outline-offset: 2px` | `apps/desktop/src/App.css:32, 71-78` |
| P2-12 | Add `aria-pressed` to `ShellModeToggle` buttons; add `role='radiogroup'` wrapper; add keyboard navigation to `role='menu'` panels in `ProjectBanner.tsx`, `CommandRail.tsx`, `PreviewSurface.tsx` | `apps/desktop/src/shell/ShellModeToggle.tsx`, `apps/desktop/src/app/ProjectBanner.tsx`, `apps/desktop/src/shell/CommandRail.tsx` |
| P2-13 | Add `tabIndex={0}` and `aria-label` to the canvas timeline element; add keyboard seek commands (arrows, J/K/L); expose playhead position via `aria-live` region | `apps/desktop/src/timeline/TimelineSurface.tsx:677` |
| P2-14 | Raise `--color-text-disabled` from `#6B7280` to at least `#8A8F9A` in `tokens.css` | `apps/desktop/src/ui/tokens.css:58` |
| P2-15 | Add `aria-label` to `Composer.tsx` send/stop buttons and `ApprovalCard.tsx` allow/deny buttons; add `aria-live='assertive'` to error div | `apps/desktop/src/agent/Composer.tsx`, `apps/desktop/src/agent/ApprovalCard.tsx` |
| P2-16 | Add project-root confinement check to Python MCP `asset_path` parameter in `_server.py` | `python/packages/montage-mcp/src/montage_mcp/_server.py:51` |
| P2-17 | Add `PEXELS_API_KEY` section and `X_BEARER_TOKEN` section to `RESOLVE_AT_STARTUP`; document both in `README.md` optional-features section | `apps/desktop/src-tauri/src/secrets.rs`, `README.md` |
| P2-18 | Change `PowerPreference::HighPerformance` to `PowerPreference::None` for interactive GPU preview renderer; reserve `HighPerformance` for batch export | `crates/render-gpu/src/lib.rs:234-238`, `crates/render-gpu/src/transform_compositor.rs:58-62` |
| P2-19 | Share a single `wgpu::Device` across all `GpuTransitionRenderer` instances instead of one device per shader | `apps/desktop/src-tauri/src/commands/preview.rs:150-167`, `crates/render-gpu/src/lib.rs` |
| P2-20 | Replace `pollster::block_on` in `GpuTransitionRenderer::new()` with `tokio::task::spawn_blocking` | `crates/render-gpu/src/lib.rs:228`, `apps/desktop/src-tauri/src/commands/preview.rs:155` |
| P2-21 | Add `TimelineRawStreamGpu` backend to the actual render spec path, or force `TimelineFfmpegReencode` until the GPU path is wired | `crates/render/src/timeline.rs:10884-10888` |
| P2-22 | Add `*.pptx`, `*.docx` to `.gitignore`; confirm `*.pyc` glob covers `skills/` depth | `.gitignore` |
| P2-23 | Add `CHANGELOG.md` entry for pyannote model CC-BY-4.0 attribution; add `cargo license --json` scan output to `THIRD_PARTY_NOTICES.md` | `THIRD_PARTY_NOTICES.md` |
| P2-24 | Log startup warning for each empty OAuth credential group in `social-server/src/main.rs` | `crates/social-server/src/main.rs:194-201` |
| P2-25 | Enforce `SubAgent.max_iterations` in the sub-session runner when the delegate tool is wired; add TODO comment linking to enforcement site | `crates/core/src/subagent.rs:43` |
| P2-26 | Map token usage from codex bridge into an `Item::UsageUpdate` variant; display turn counter and token count in the chat rail | `crates/codex-bridge/src/mappers.rs`, `apps/desktop/src/` |
| P2-27 | Add Hardware Requirements section to `README.md` (macOS 12+ Metal, Linux with Vulkan for GPU transitions) | `README.md` |
| P2-28 | Add GitHub Discussions or Discord link to `README.md`; add contact email to `MAINTAINERS.md` | `README.md`, `MAINTAINERS.md` |
| P2-29 | Add `good first issue` / `help wanted` label guidance to `CONTRIBUTING.md` | `CONTRIBUTING.md` |
| P2-30 | Add a data-subject rights section (account deletion, data export) to the social-accounts settings panel and a server-side deletion endpoint | `apps/desktop/src/app/SettingsModal.tsx`, `crates/social-server/src/` |

---

## Per-Dimension Findings

---

### Licensing & Legal

The Apache-2.0 root license is clean, all first-party crates inherit it correctly, and THIRD_PARTY_NOTICES.md covers the main dependencies. The critical gap is the vendored codex fork's deliberate client-version spoofing, which creates an OpenAI ToS exposure that makes a public repository legally risky. Secondary gaps include no SPDX file headers, no CLA/DCO, and incomplete THIRD_PARTY_NOTICES attribution for OpenSSL and the pyannote model.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| Blocker | Vendored code spoofs OpenAI client version `0.128.0` to bypass model gates | `vendor/codex-rs/login/src/auth/default_client.rs`: `let build_version = "0.128.0";`; `vendor/codex-rs/SOURCE` lines 22-27 document the spoof | Obtain first-party OpenAI integration agreement or register a proper `client_id`; at minimum add legal disclaimer | G1, G2, G3 |
| Medium | LGPLv2 bubblewrap C code statically compiled into Linux binary with no relinking mechanism documented | `vendor/codex-rs/bwrap/build.rs:51-68`; `vendor/codex-rs/vendor/bubblewrap/COPYING` is LGPLv2 | Add LGPLv2 relinking obligation to THIRD_PARTY_NOTICES.md; document that source is available in the vendored tree (satisfies LGPLv2 §6(b)) | G1, G3 |
| Medium | No `cargo-deny` or `cargo-about` license check at workspace root in CI | No `deny.toml` at repo root; no `cargo deny` step in `ci.yml` | Add `deny.toml` at repo root and `cargo deny check licenses` to CI | G1, G2, G3 |
| Low | No SPDX or copyright headers in any first-party Rust source files | `find crates -name '*.rs'` returns 446 files; `grep -r Copyright` returns 2 unrelated hits | Add `// SPDX-License-Identifier: Apache-2.0` to all `.rs` files | G1, G2 |
| Low | No CLA or DCO requirement for external contributors | `CONTRIBUTING.md` has no mention of CLA or DCO; no PR template checkbox | Add DCO requirement; configure DCO GitHub Action | G1, G2 |
| Low | HuggingFace gated model (pyannote diarization, CC-BY-4.0) not documented in THIRD_PARTY_NOTICES | `THIRD_PARTY_NOTICES.md` has no pyannote entry; model is `pyannote/speaker-diarization-community-1` | Add 5-line CC-BY-4.0 attribution entry to THIRD_PARTY_NOTICES.md | G1, G3 |
| Low | Compiled `.pyc` files present in `skills/` but not reliably gitignored on all git versions | `skills/viral-clip-extractor/scripts/__pycache__/*.pyc` on disk | Add `*.pyc` as standalone gitignore rule | G1 |
| Low | OpenSSL dependency not mentioned in THIRD_PARTY_NOTICES | `Cargo.toml`: `openssl-sys = "*"`; THIRD_PARTY_NOTICES has no OpenSSL entry | Run `cargo license --json`; update THIRD_PARTY_NOTICES; consider switching to `aws-lc-rs` | G1, G3 |

---

### OSS Community & Docs

The foundational community files are all present and well-written (README, CONTRIBUTING, CODE_OF_CONDUCT, SECURITY, AGENTS.md, PR/issue templates). The main gaps are a README that describes packaging as broken, developer-only prerequisites that contradict the G3 GUI goal, and missing user-facing documentation.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| Blocker | README admits release packaging is broken with no ETA or workaround | `README.md:140-142`; `CONTRIBUTING.md:49-51` | Restore packaging pipeline or clarify audience; remove broken-packaging language | G1, G3 |
| Medium | README prerequisites are developer-only (Rust, Node, Python, ffmpeg, API keys) | `README.md:16-23` — no mention of ChatGPT subscription or GUI installer | Add "For End Users" section describing the downloadable installer flow | G1, G3 |
| Medium | README and SECURITY.md call project "early-stage" / "local-first" | `README.md:5`; `SECURITY.md:3` | Update framing to describe current capabilities accurately before public launch | G1, G3 |
| Medium | No crate-level architecture overview for contributors | No ARCHITECTURE.md; CONTRIBUTING.md has no architecture pointer | Add ARCHITECTURE.md with one-paragraph per major crate and ASCII data-flow diagram | G1 |
| Low | No CHANGELOG, version history, or public roadmap | `find . -maxdepth 1 -name 'CHANGELOG*'` returns nothing; `git tag --list` returns nothing | Add CHANGELOG.md and adopt `v*` tag discipline | G1 |
| Low | README does not link to CONTRIBUTING.md, CODE_OF_CONDUCT.md, or SECURITY.md | `grep 'CONTRIBUTING\|CODE_OF_CONDUCT\|SECURITY' README.md` returns no matches | Add "Contributing" section to README.md with explicit links | G1 |
| Low | No community support channel or contact point for end-users | No Discord/Discussions/Matrix link anywhere in docs | Add GitHub Discussions or Discord invite to README | G1, G3 |
| Low | No "good first issue" guidance for new contributors | CONTRIBUTING.md has no starter-issue pointers | Add "Where to start" section to CONTRIBUTING.md; create `good first issue` label | G1 |

---

### Secrets & Repo Hygiene

No real API keys or private key material are present in the tracked tree. The OS-keychain secret resolution is well-designed. The gaps are a hardcoded AEAD key default and a committed Supabase project reference — both fixable before G1 launch.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| High | Cryptographic AEAD key hardcoded in `run-local.sh` | `crates/social-server/run-local.sh:53`: `export SOCIAL_TOKEN_AEAD_KEY="${SOCIAL_TOKEN_AEAD_KEY:-c2e3e7c14a025d829f0411ce456f1ca01b3f546218bf21892b3dd7cc0d9efedd}"` | Remove default value; add `exit 1` when unset; rotate key in any live deployment | G1, G2 |
| Low | Live Supabase project ref committed in two tracked files | `run-local.sh:76`; `docs/social-server/README.md:124-125`: ref `vgkocfbtkzmpklruqmsx` | Scrub ref from history with `git filter-repo` or BFG before public push | G1, G2 |
| Low | Predictable dev bearer tokens committed as script defaults | `run-local.sh:45,48`: `DESKTOP_AUTH_TOKEN=local-dev-token`, `SERVICE_SHARED_SECRET=local-internal-token` | Fail-fast on empty/default values; remove hardcoded literals from curl examples | G1, G2 |
| Low | `AI@ISU Innovation Challenge 2.0.pptx` is untracked but not gitignored | `git status` shows `?? 'AI@ISU Innovation Challenge 2.0.pptx'`; `git check-ignore` returns exit 1 | Add `*.pptx` (and `*.docx`, `*.xlsx`) to `.gitignore` | G1 |
| Low | No `.gitattributes` — binary assets accumulate without LFS | 73 tracked binary files (PNG, JPG, WAV, MP4) confirmed via `git ls-files`; no `.gitattributes` at root | Add `.gitattributes` marking `*.png`, `*.jpg`, `*.wav`, `*.mp4` as binary; migrate large files to Git LFS | G1 |
| Low | Social server boots silently with empty OAuth client credentials | `crates/social-server/src/main.rs:194-201`: all OAuth credentials use `unwrap_or_default()` | Log `tracing::warn!` for each empty credential group at startup | G2 |
| Low | No automated secret scanning in CI | No `trufflehog` / `gitleaks` step in any workflow file | Add `gitleaks` or `trufflehog` step to CI on every PR | G1, G2 |
| Medium | Demo podcast JPEG assets lack provenance and license documentation | `apps/desktop/src/shell/assets/` contains 15 JPEG files; THIRD_PARTY_NOTICES.md has no image entries | Document origin of each image; replace with CC0/public-domain images if uncertain | G1, G3 |

---

### Security Posture

The codebase has thoughtful path-traversal defenses in the Rust MCP tool layer, a well-scoped Tauri asset-protocol, and a macOS seatbelt sandbox that passes its unit tests. The critical gap is that the sandbox is never invoked at runtime — it is dead code.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| High | Sandboxing crate is never invoked at runtime — all tool calls run unsandboxed | `grep -rn 'montage_sandboxing' crates/core/src/` returns zero results; `SandboxFirst` policy never overridden; `start_render.rs` spawns ffmpeg directly | Wire `Sandbox::run()` into render job spawn path; or document approval gate as primary control and retire dead `SandboxFirst` machinery | G2, G3 |
| High | No `cargo-deny` or `cargo-audit` in CI — known CVEs go undetected | `find . -maxdepth 3 -name 'deny.toml' | grep -v vendor` returns nothing; no `cargo audit` in `ci.yml` | Add `deny.toml` at workspace root; add `cargo deny check advisories` and `cargo audit` to CI | G1, G2, G3 |
| Medium | Auth tokens default to plaintext file storage (`~/.codex/auth.json`) not OS keychain | `crates/auth/src/env.rs:100`: `store_mode: AuthCredentialsStoreMode::default()` → `File` variant | Change default to `Auto` (keychain-first) for the Tauri desktop launch path | G2, G3 |
| Medium | `generate_thumbnails_for_asset` joins project root with unvalidated `asset_id` — path traversal | `apps/desktop/src-tauri/src/commands/thumbnail.rs:174`: `let abs = project_root.join(&asset_id);` — no bounds check | Add `if !abs.starts_with(&project_root) { return Err(...) }` | G2, G3 |
| Low | `list_thumbnail_frames` accepts arbitrary directory path with no project-root confinement | `commands/thumbnail.rs:132`: `pub async fn list_thumbnail_frames(dir: String)` with no confinement check | Add project-root confinement check; two lines | G2, G3 |
| Low | Python MCP indexers accept absolute `asset_path` with no path confinement | `python/packages/montage-mcp/src/montage_mcp/_server.py:51`: `asset_path: str` passed directly to ffmpeg/whisperx | Add `os.path.realpath(req.asset_path).startswith(os.path.realpath(req.project_root))` check | G2, G3 |
| Low | Tauri CSP allows `unsafe-inline` for `style-src` | `apps/desktop/src-tauri/tauri.conf.json:26`: `style-src 'self' 'unsafe-inline'` | Replace with nonce-based CSP for dynamic styles | G2, G3 |
| Low | Linux sandbox is a documented stub — sandboxing not implemented on Linux | `crates/sandboxing/src/lib.rs:107`: `LinuxNotImplemented` variant; `run()` returns this on Linux | Implement bubblewrap+landlock before any Linux G3 build; document gap in README | G2 |

---

### Build, Packaging & Signing

The yt-dlp sidecar is correctly bundled as a working model. Icon assets are complete. The fatal gap is the entire release pipeline: no `tauri build` in CI, no signing, no notarization, no updater, and ffmpeg/montage-mcp-server not declared as sidecars.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| Blocker | No macOS code signing or notarization configuration | `tauri.conf.json` has no `macOS` signing block; zero matches for `APPLE_CERTIFICATE`, `APPLE_ID`, `notarize` across repo | Obtain Apple Developer ID; configure signing in `tauri.conf.json`; add notarization secrets to GitHub Actions | G3 |
| Blocker | No Windows Authenticode signing configuration | Zero matches for `WINDOWS_CERTIFICATE`, `TAURI_PRIVATE_KEY`, `Authenticode` across repo | Obtain EV certificate; configure `TAURI_SIGNING_PRIVATE_KEY` in release workflow | G3 |
| Blocker | ffmpeg/ffprobe assumed system-installed — will silently fail for all non-technical users | `crates/render/src/ffmpeg.rs:6-7`: "We do not bundle a static ffmpeg binary — that's a v2 packaging concern"; `tauri.conf.json` `externalBin` lists only `yt-dlp` | Bundle static ffmpeg+ffprobe as Tauri `externalBin` sidecars using yt-dlp pattern | G3 |
| Blocker | Python/uv indexers require system `uv` + Python — unbundled 1.3+ GB dependency | `crates/config/src/defaults.rs:208-226`: `uv_command()` walks PATH; ~3 GB venv on first sync | Ship vendored `uv` binary + pre-built venv tarball in `resources/`, or move to hosted API | G3 |
| High | Tauri updater plugin not configured — no in-app update path | `apps/desktop/src-tauri/Cargo.toml`: `tauri-plugin-updater` absent; `tauri.conf.json` has no `updater` key; `upgrade_cmd.rs:24` explicitly errors | Add `tauri-plugin-updater`, configure endpoint and keypair | G3, G2 |
| High | `bundle.targets = 'all'` but `tauri build` never run in CI | `tauri.conf.json:35`: `"targets": "all"`; `ci.yml` desktop job runs only `pnpm build` (Vite only) | Add `tauri build --debug` step to CI for at least macOS | G3, G1 |
| Medium | App identifier uses placeholder domain `com.montage.desktop` | `apps/desktop/src-tauri/tauri.conf.json:4`: `"identifier": "com.montage.desktop"` | Register real domain-backed bundle ID in Apple developer portal | G3, G1 |

---

### Release Pipeline & CI

CI runs format, clippy, and tests on macOS and Linux with good caching. The entire release side is absent: no version tags, no release workflow, no signed artifacts, no Windows CI coverage.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| Blocker | Zero release/build/sign/publish workflow — no downloadable artifacts ever produced by CI | `.github/workflows/` contains only `ci.yml` and `evals.yml`; neither invokes `tauri build`; `CONTRIBUTING.md` confirms packaging is broken | Create `release.yml` triggered on `v*` tags; build, sign, and upload artifacts per platform | G3 |
| Blocker | No code-signing or notarization configuration anywhere | `tauri.conf.json` has no `bundle.macOS`, `bundle.windows`; `grep -rn 'sign\|notariz\|APPLE_CERTIFICATE\|windowsSign'` across `.github/` and `src-tauri/` returns zero | Obtain certs; add Tauri signing config; store in GitHub secrets | G3 |
| High | No git tags, no CHANGELOG, no documented versioning process | `git tag --list` returns zero tags; no CHANGELOG.md; all crates at `version = '0.1.0'` | Adopt `v*` tag discipline; add CHANGELOG.md; use `cargo-release` or `release-plz` | G1, G3 |
| High | Windows entirely absent from CI matrix | `rust` job matrix: `os: [macos-latest, ubuntu-latest]`; no `windows-latest` anywhere | Add `windows-latest` to `rust` and `desktop` job matrices | G2, G3 |
| High | Tauri native binary never built by CI — only Vite frontend is compiled | `ci.yml` desktop job runs `pnpm build` = `tsc && vite build`; Tauri Rust backend excluded from test step at line 66 | Add `tauri-build` job running `pnpm tauri build --debug` on macOS and Linux | G2, G3 |
| Medium | No documented branch-protection or required status checks | No branch-protection config; commits merging directly to `main` per git log | Enable GitHub branch protection requiring CI checks to pass before merge | G1, G2 |
| Medium | No `cargo audit` or `cargo deny` step — known CVEs go undetected | `grep -rn 'cargo-audit\|cargo deny\|RustSec'` across `.github/workflows/` returns zero matches | Add `cargo audit` (via `rustsec/audit-check`) and `cargo deny check advisories` to CI | G1, G2 |
| Low | 227-crate vendored codex-rs fork inflates CI compile time | `Cargo.toml` workspace members include 227 `vendor/codex-rs/*` entries; `cargo clean` runs between clippy and test steps | Restructure Linux job to run `cargo clean` only once, after doc step | G2 |

---

### End-User Auth / Login UX

The ChatGPT OAuth flow is fully implemented end-to-end — the most important G3 prerequisite. The gaps are polish items: no auth gate on first launch, no re-login prompt on token expiry, and auth status not fetched on startup.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| Blocker | No auth gate on first launch — user can reach full app without signing in | `Landing.tsx` has no auth entry point; `WelcomeCard.tsx` has no sign-in step; `useAuth.open()` only accessible from `SettingsModal.tsx:143` | Auto-open `AuthChooser` if `auth_status.mode === 'none'` on first launch; block agent input | G3 |
| High | Token refresh failures surface as raw error toast, not as re-login prompt | `vendor/codex-rs/login/src/auth/manager.rs:88-93`; turn-end error lands in `commandError` toast (`App.tsx:2394`); `AuthChooser` never auto-opens on session expiry | Detect refresh-failure substrings in turn-end handler; auto-open `AuthChooser` | G2, G3 |
| High | ChatGPT sign-in reuses codex's first-party OAuth client ID without OpenAI authorization | `crates/auth/src/lib.rs:16-19` documents the reuse; client id `app_EMoamEEZ73f0CkXaXp7hrann` used for both authorize and refresh | Register first-party Montage OAuth client with OpenAI or obtain explicit written permission | G2, G3 |
| Medium | AuthChooser has no entry point from the Landing / project-manager screen | `Landing.tsx` renders New Project, Open Project, Import Media — none open `AuthChooser` | Add "Sign in" entry point to Landing sidebar or `WelcomeCard` footer | G3 |
| Low | Auth status not fetched on startup — UI shows "Not signed in" even with valid credentials | `state/auth.ts` `refresh()` not called in `startupHydration.ts`; status initialises to `null` | Call `useAuth.getState().refresh()` once in `startupHydration.ts` | G3 |
| Low | ChatGPT OAuth relies on system browser — no in-app webview path | `vendor/codex-rs/login/src/server.rs:166-168`: `webbrowser::open(&auth_url)`; fallback is manual URL copy | Test fallback path on Windows/Linux; add explicit error when both ports 1455/1457 are busy | G3 |
| Low | Sign-out button fires immediately with no confirmation | `AuthChooser.tsx:137-141`: calls `logout()` directly on click; `auth_logout` revokes token server-side | Add single confirmation step before calling `auth_logout` | G3 |
| Low | API key validation error proxies raw Rust error string to user | `AuthChooser.tsx:56-58`: regex strips prefix but falls back to full Rust error | Define explicit user-facing validation message in `AuthChooser` | G3 |

---

### Non-Technical User Readiness

The GUI ChatGPT sign-in flow is the standout G3 strength. Everything else — ffmpeg, Python stack, montage-mcp-server, multiple hidden API keys, first-run experience — requires significant packaging work.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| Blocker | No release/packaging pipeline — no installer can be built | `README.md:138-142`; no `tauri build` in CI; no signing configuration | Add GitHub Actions release workflow with `tauri build`, signing, and artifact upload | G3 |
| Blocker | ffmpeg not bundled — absent produces developer-oriented error | `crates/render/src/ffmpeg.rs:7, 44-47`: "We do not bundle…"; error: "install via your package manager"; `readiness.ts:184` maps to "FFmpeg unavailable" | Bundle static ffmpeg as Tauri `externalBin`; update readiness error message | G3 |
| Blocker | Python 3.11 + uv + indexer packages must be pre-installed | `crates/config/src/defaults.rs:15-26`; `python/SMOKE.md:88`: "downloads ~3GB of Python dependencies"; no bundling in `tauri.conf.json` | Evaluate PyInstaller/Nuitka per indexer, or ship vendored `uv` + pre-built venv in `resources/` | G3 |
| Blocker | montage-mcp-server not declared as Tauri sidecar | `apps/desktop/src-tauri/src/codex_session.rs:97-100`; `tauri.conf.json` `externalBin` lists only `yt-dlp` | Add `montage-mcp-server` to `tauri.conf.json` `externalBin`; fix error message | G3, G2 |
| High | ANTHROPIC_API_KEY required for editorial-moments and topic indexers — no GUI path | `python/packages/editorial-moments-mcp/src/editorial_moments_mcp/__init__.py:364-369`; `SettingsModal` has no Anthropic key field | Surface Anthropic key entry in Settings modal with cost disclosure (~$0.05/hr) | G3, G2 |
| Medium | Speaker diarization requires HuggingFace account, EULA acceptance, and HF_TOKEN | `python/SMOKE.md:96-97`; no GUI path for `HF_TOKEN` setup | Add HF_TOKEN field to Settings modal with link to EULA acceptance page | G3 |
| Medium | Zero end-user documentation — all docs are contributor/developer oriented | `find . -maxdepth 2 -name '*.md'` shows only developer docs; no user guide, no FAQ | Create `docs/user-guide/` with Getting Started, indexing explainer, agent usage, troubleshooting | G1, G3 |
| Medium | App launches into usable-looking state with no credentials configured | `App.tsx:2347-2350`: `AuthChooser` and `WelcomeCard` mount as siblings; neither blocks main UI; `start_turn` fails with raw error string | On `mode === 'none'`, open `AuthChooser` automatically; block agent input field | G3 |

---

### Testing & Reliability

The render crate has comprehensive integration tests, auth has solid unit coverage, and the wiremock-backed social tests are well-structured. The major gap is the untested MCP tool surface (103 of 111 tools) and the completely empty eval crate.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| Blocker | montage-eval crate directory is empty — evals.yml fails on every run | `ls crates/eval/` shows only `.` and `..`; `evals.yml:46,49,111` call `cargo run -p montage-eval` | Scaffold `crates/eval/` with `main.rs` handling all flags; add to workspace `Cargo.toml` | G2, G3 |
| High | 103 of 111 MCP tool implementations have zero tests | `find crates/core/src/montage_mcp/tools -name '*.rs' | xargs grep -L '#\[test\]' | wc -l` = 103; `verify_render.rs` (1,758 lines), `export_package.rs` (1,588 lines), `apply_edl.rs` (412 lines) untested | Prioritize unit tests for timeline-mutation tools, output tools, and generated-media pipeline | G1, G2 |
| High | No CI job builds or smoke-tests the downloadable installer | Neither `ci.yml` nor `evals.yml` calls `tauri build`; `Makefile` has no bundle target | Add workflow that runs `cargo tauri build` on macOS and Linux, saves artifacts, runs headless smoke test | G3 |
| High | 21,000+ lines of Tauri command handlers have zero tests | `apps/desktop/src-tauri/src/commands/` 43 files totaling 21,258 lines; `auth.rs`, `proposal.rs`, `timeline.rs`, `media.rs`, `transcode.rs` all have zero test coverage | Add unit tests for `auth_status`, `auth_set_api_key`, proposal accept/reject, `import_media` | G2, G3 |
| High | codex-bridge `launch`, `start_turn`, `respond_approval`, `interrupt` are untested | `crates/codex-bridge/src/lib.rs` (1,062 lines); 6 tests cover only data mapping; critical async methods have zero tests | Add integration tests using `vendor/codex-rs/app-server-test-client`; add unit tests for `respond_approval` branching paths | G2, G3 |
| Medium | Index/Python end-to-end test is permanently ignored and not run in regular CI | `crates/index/tests/end_to_end.rs:77`: `#[ignore = "requires python workspace synced"]`; real dispatch path never tested in CI | Run `cargo test --workspace -- --ignored` in a scheduled job with `uv sync` | G2 |
| Low | Render job integration test uses timing-sensitive 100ms poll loop | `crates/render/src/job.rs:979-989`: polls `m.status()` every 100ms for up to 15s | Replace sleep-poll with channel notification or bounded retry; double timeout | G2 |
| Low | `live_` social tests named like real-network tests but backed by wiremock — naming convention mismatch | `crates/social/src/instagram_upload.rs:776-814`; `tiktok_upload.rs:921-998`: `live_*` functions without `#[ignore]` | Rename wiremock-backed tests or add `#[ignore]` and document the convention | G2 |

---

### Privacy, Data Handling & Telemetry Consent

No telemetry or analytics infrastructure was found — a genuine strength. The critical gaps are the complete absence of a privacy policy and any in-app consent mechanism before data leaves the device.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| High | No Privacy Policy or Terms of Service document exists | `find . -maxdepth 6 -iname 'privacy*' -o -iname 'terms*' -o -iname 'tos*'` returns zero results | Create `PRIVACY.md` enumerating data egress (OpenAI, Deepgram, OpenRouter, social platforms); link from AuthChooser and WelcomeCard | G1, G2, G3 |
| High | No in-app disclosure that user transcript content is sent verbatim to OpenAI | `crates/codex-bridge/src/lib.rs:428-448`: `start_turn()` sends transcripts as `UserInput::Text`; `AuthChooser.tsx` and `WelcomeCard.tsx` contain zero mention of transcripts or data egress | Add one-sentence data-flow notice to `AuthChooser` before first authentication | G2, G3 |
| High | Deepgram cloud transcription silently sends raw audio to third party with no user notice | `python/packages/whisper-mcp/src/whisper_mcp/__init__.py:114,120,262`: posts audio bytes to `https://api.deepgram.com/v1/listen` when `DEEPGRAM_API_KEY` is set | Add one-time consent prompt before first Deepgram use; provide UI switch to local Whisper | G2, G3 |
| High | No mechanism for data-subject rights (deletion, export, opt-out) | Exhaustive search of `apps/desktop/src/` for `delete.*account`, `export.*data`, `right.*erasure` returns no results; social server on Fly persists OAuth tokens with no deletion endpoint | Add account-deletion Tauri command; document OpenAI's own data-deletion tools | G2, G3 |
| Medium | User editorial briefs and transcript snippets forwarded to OpenRouter for video generation with no disclosure | `crates/core/src/generated_media/openrouter.rs:88-111`: assembled prompt sent verbatim to `https://openrouter.ai/api/v1/videos` | Document OpenRouter as data processor in privacy policy; list all third-party data recipients in settings | G2, G3 |
| Medium | No consent gate — app starts processing media on first launch without disclosures | `WelcomeCard.tsx` shows three marketing bullets and single "Get started" button; no checkbox, no privacy policy link | Replace "Get started" with two-step flow: data-flow summary + "I understand and agree" | G2, G3 |
| Low | Index-timing `telemetry` field with no opt-out path or disclosure documentation | `apps/desktop/src-tauri/src/commands/index.rs:475`: local timing data only; no remote telemetry found | Add comment clarifying local-only; include explicit "no analytics" statement in privacy policy | G1, G2 |

---

### Model/AI Cost Controls & Runaway Agent Spend

The codex-rs goals subsystem and `SubAgent.max_iterations` exist but neither is wired at runtime. No per-session spend controls or token usage visibility reach the user.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| Medium | Primary thread manager has no per-session turn cap — unbounded API spend possible | `vendor/codex-rs/core/src/thread_manager.rs` (1543 lines): `grep max_turns/turn_limit/TURN_CAP` returns zero non-test hits | Add configurable `max_turns: Option<usize>` to `ThreadManagerState`; default ~40; surface in Settings | G2, G3 |
| Medium | Bridge never sets a `token_budget` on thread/goal — server-side budget enforcement never activated | `crates/codex-bridge/src/lib.rs:367-375`: `ThreadStartParams::default()` has no goal or budget; `grep 'token_budget\|ThreadGoalSet'` in `apps/desktop/src-tauri/src/` returns zero hits | Issue `ThreadGoalSetParams` with default budget immediately after `thread/start` | G2, G3 |
| Medium | Token usage tracked in backend but never surfaced to user | `crates/core/src/events.rs:78-87`: `Usage` struct defined; `crates/codex-bridge/src/mappers.rs` maps zero usage fields; `apps/desktop/src/` has no cost/usage component | Map `TokenUsage` events through bridge as `Item::UsageUpdate`; add turn/token counter to chat rail | G2, G3 |
| Medium | Settings and Check for Updates menu items are permanently disabled stubs | `apps/desktop/src-tauri/src/app_menu.rs:48-49`: `DISABLED_SETTINGS` and `DISABLED_UPDATES` built via `disabled()` helper; never re-enabled | Implement minimal Settings screen; wire `app:settings`; point `app:check_updates` at GitHub Releases | G2, G3 |
| Medium | No per-turn cost estimate shown for API-key users | `AuthChooser.tsx:102`: static string only; `mappers.rs` forwards no cost data; `state.ts` has no usage field | After each turn, emit cumulative token count; display estimated cost badge; warn at configurable threshold | G2, G3 |
| Low | `SubAgent.max_iterations` declared but never enforced — dead code | `crates/core/src/subagent.rs:43`: field assigned but no read site exists anywhere in the project | Add loop counter enforcement before wiring delegate tool; add unit test verifying cap fires | G2 |

---

### GPU/Driver Portability & Resource Footprint

The GPU preview pipeline uses wgpu correctly with graceful adapter enumeration. The main issues are resource-efficiency gaps (one wgpu Device per shader, HighPerformance on battery, blocking init on async thread) rather than hard failures.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| Medium | `PowerPreference::HighPerformance` unconditionally selects discrete GPU on multi-GPU laptops | `crates/render-gpu/src/lib.rs:234-238`; `transform_compositor.rs:58-62`: no env-var escape hatch | Change to `PowerPreference::None` for interactive preview; reserve `HighPerformance` for batch export | G2, G3 |
| Medium | No CPU/software render fallback when wgpu returns `NoAdapter` | `crates/render-gpu/src/lib.rs:240`: `NoAdapter` → `GpuError::NoAdapter`; `preview.rs:160-163` maps to error string; `useGpuTransitionPreview.ts:140-144` returns null silently | Document CSS dissolve fallback; implement CPU alpha-lerp for export path | G2, G3 |
| Medium | `TimelineRawStreamGpu` selected in metadata but export still runs FFmpeg | `crates/render/src/timeline.rs:10884-10888`: `select_timeline_render_backend_evidence()` result stored but `build_timeline_render_spec_inner` always produces FFmpeg argv | Wire GPU backend into spec or force `TimelineFfmpegReencode` until path is active | G2, G3 |
| Low | No minimum hardware requirements documented | `README.md:15-21` lists only software prerequisites; no GPU or OS-version requirement mentioned | Add Hardware Requirements section to README.md | G1, G3 |
| Low | One wgpu Device created per `TransitionShader` — wastes VRAM | `preview.rs:150-167`: separate `wgpu::Device` per shader type; each calls `request_adapter`/`request_device` independently | Share single `Arc<wgpu::Device>` across all shader renderers | G2, G3 |
| Low | `pollster::block_on` used to await wgpu init inside async Tauri command | `crates/render-gpu/src/lib.rs:228`: `pollster::block_on(Self::new_async(shader))` on Tokio async thread without `spawn_blocking` | Use `tokio::task::spawn_blocking`; or expose async constructor and `await` properly | G2 |

---

### Accessibility (a11y) Completeness

The component library uses semantic HTML in many places. The main accessibility gaps are the canvas timeline (entirely inaccessible to screen readers), missing focus trapping in modals, and an inadequate focus ring contrast ratio in the legacy CSS layer.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| Medium | `App.css` focus ring is semi-transparent red yielding 1.6:1 contrast — below 3:1 WCAG minimum | `apps/desktop/src/App.css:32`: `--focus-ring: 0 0 0 2px rgba(239,68,68,0.36)`; lines 71-78 apply it to `button:focus-visible` | Replace with `outline: 2px solid var(--color-border-focus); outline-offset: 2px` | G2, G3 |
| Medium | Modals have `aria-modal` but no focus trapping — Tab escapes into inert background | `SettingsModal.tsx:99-110`: only Escape trapped via `document.addEventListener`; no Tab cycle; `ManageProjectsDialog.tsx` lacks `role='dialog'` entirely | Use `focus-trap-react` or implement manual Tab cycle in each modal | G2, G3 |
| Low | Canvas timeline has zero screen-reader representation | `TimelineSurface.tsx:677`: `<canvas ... />` with no `aria-label`, `role`, `tabIndex`, or fallback text | Add `tabIndex={0}`, `aria-label`, `aria-live` playhead region; implement keyboard seek commands | G2, G3 |
| Low | `--color-text-disabled` (`#6B7280`) fails WCAG AA 4.5:1 on all dark surfaces | `apps/desktop/src/ui/tokens.css:58`: computed contrast 3.81:1 on `#141414` | Raise `--color-text-disabled` to at least `#8A8F9A` | G2, G3 |
| Low | `role='menu'` panels lack ArrowUp/ArrowDown keyboard navigation | `ProjectBanner.tsx:121`; `CommandRail.tsx:1603`; `PreviewSurface.tsx:431`: no `onKeyDown` handling in any menu container | Implement APG menu keyboard interaction model; or replace with Radix UI `DropdownMenu` | G2, G3 |
| Low | `ShellModeToggle` buttons have no `aria-pressed` or `aria-current` | `ShellModeToggle.tsx:22-34`: visual-only active state; zero aria attributes | Add `aria-pressed={mode === m}` to each button; add `role='radiogroup'` wrapper | G1, G2 |
| Low | Legacy `Composer.tsx` and `ApprovalCard.tsx` buttons have no `aria-label` | `Composer.tsx:104,108`; `ApprovalCard.tsx:64-84`: no aria attributes | Add descriptive `aria-label` to Send/Stop and Allow/Deny buttons; add `aria-live='assertive'` to error div | G2, G3 |
| Low | Pane resize separators lack ARIA slider values and keyboard operability | `StageShell.tsx:353-354, 406-407, 485-487`: `role='separator'` with no `aria-valuenow/min/max` or `tabIndex` | Add `tabIndex={0}`, ARIA value attributes, and arrow-key handler to each `PanelResizeHandle` | G2, G3 |

---

### Auto-Update Safety, Rollback & Update UX

The auto-update infrastructure is entirely absent at all layers: no Cargo dependency, no Tauri plugin registration, no JSON configuration, no signing keypair, and no CI release workflow.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| Blocker | No CI release/publish workflow — no path from commit to downloadable installer | `.github/workflows/` contains only `ci.yml` and `evals.yml`; neither runs `tauri build` or produces artifacts | Create `release.yml` triggered on `v*` tags; build, sign, upload for macOS + Windows | G1, G2, G3 |
| High | `tauri-plugin-updater` not declared and no updater endpoint configured | `apps/desktop/src-tauri/Cargo.toml`: only `tauri-plugin-opener`, `tauri-plugin-dialog`, `tauri-plugin-shell`; `tauri.conf.json` has no `plugins.updater` key | Add `tauri-plugin-updater`; configure endpoint and `pubkey`; generate signing keypair | G2, G3 |
| High | No update-signature keypair and no GitHub Actions secret scaffolding | `grep -r TAURI_SIGNING` across repo returns no output; no `release.yml` | Run `tauri signer generate`; store private key as GitHub secret; embed public key in `tauri.conf.json` | G2, G3 |
| High | No mechanism to force-update users away from a vulnerable version | `tauri.conf.json` has no updater configuration; `tauri-plugin-updater` absent; no `required` flag infrastructure | When implementing updater, add `minVersion` field to update manifest and enforce in plugin | G2, G3 |
| Medium | "Check for Updates..." menu item is permanently disabled and ships that way | `app_menu.rs:49, 158`: `disabled()` helper sets `enabled: false` unconditionally; never referenced from frontend | Remove menu item or point it at GitHub Releases page via `tauri-plugin-opener` | G2, G3 |
| Medium | No rollback path if a bad update ships | `tauri.conf.json` has no updater or installer format configuration | Choose NSIS on Windows (supports rollback) and versioned `.pkg` on macOS; document rollback procedure | G2, G3 |
| Low | `app:check_updates` absent from frontend `MENU_COMMANDS` type map | `apps/desktop/src/app/menuCommands.ts:3-36`: 31 entries but `app:check_updates` is absent; `isMenuCommandId` guard would swallow the event | Add `CHECK_FOR_UPDATES: 'app:check_updates'` to `MENU_COMMANDS` at same time menu item is re-enabled | G2, G3 |

---

### Third-Party API Key Requirements Hidden from Non-Technical Users

Several valuable features require separate paid accounts and API keys that are not surfaced in the GUI, not documented in the README, and some are not even propagated correctly to subprocesses.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| High | Pexels API key has zero GUI setup path — b-roll feature silently requires a third-party account | `secrets.rs:46-50`: `PEXELS_API_KEY` absent from `RESOLVE_AT_STARTUP`; zero results from `grep -rn 'pexels' apps/desktop/src/app/auth/`; no Pexels section in `SettingsModal` | Add Pexels key row to `SettingsModal`; add to `RESOLVE_AT_STARTUP`; fix error message | G2, G3 |
| High | OpenRouter API key has no GUI entry — AI b-roll generation requires separate paid account with no disclosure | `grep -rn 'openrouter' apps/desktop/src-tauri/src/commands/` returns 0 results; no OpenRouter section in `SettingsModal`; `openrouter.rs:28-40` loads key from env/keychain only | Add OpenRouter section to `SettingsModal` with cost explanation and link to openrouter.ai | G2, G3 |
| High | OpenRouter account + pre-funded balance undocumented in README or onboarding | `README.md:153-154`: lists only `ANTHROPIC_API_KEY` and `HF_TOKEN`; `PEXELS_API_KEY` and `OPENROUTER_API_KEY` absent; `docs/` mentions OpenRouter only in internal design notes | Add "Optional features" section to README documenting both keys with setup steps | G1, G2, G3 |
| High | No user-visible cost estimate or confirmation gate before OpenRouter video generation | `crates/core/src/tools/start_generated_media_job.rs:150-192`: no cost estimate step; `openrouter.rs:232-233`: `cost_estimate_usd` is always `None`; `GeneratedMediaPanel.tsx`: no cost warning | Query OpenRouter `/models` endpoint before submission; populate `cost_estimate_usd`; surface in approval modal | G2, G3 |
| Medium | Missing Pexels key error message references nonexistent CLI command | `crates/core/src/tools/search_broll.rs:179-181`; `crates/core/src/montage_mcp/tools/search_broll.rs:122-124`: says `montage config set pexels`; actual command is `montage secrets-set pexels_api_key` | Update both error strings to reference `montage secrets-set pexels_api_key` | G2, G3 |
| Medium | OpenRouter per-video cost tracked as `None` and never shown to user | `openrouter.rs:232-233`: `cost_estimate_usd: None`, `cost_actual_usd: None`; `GeneratedMediaPanel.tsx:102-119`: never renders cost fields | Parse cost from OpenRouter status response; render in `GeneratedMediaPanel`; add running-total display | G2, G3 |
| Medium | `PEXELS_API_KEY` excluded from startup keychain prefetch — MCP subprocesses cannot inherit it | `secrets.rs:46-50`: absent from `RESOLVE_AT_STARTUP`; `crates/core/src/pexels.rs:111-118`: only parent process gets keychain call | Add `(env_vars::PEXELS_API_KEY, accounts::PEXELS_API_KEY)` to `RESOLVE_AT_STARTUP` slice | G2, G3 |
| Low | `X_BEARER_TOKEN` is a fourth hidden API key with no GUI path and no README entry | `crates/secrets/src/lib.rs:122,135`; absent from `RESOLVE_AT_STARTUP`; absent from `README.md:153-154` | Add to `RESOLVE_AT_STARTUP` and README optional-features section | G2, G3 |

---

### Code Quality & Hardening

Workspace-level lint enforcement (deny `unwrap_used`, `expect_used`) is a genuine strength. The main gaps are observability (silent Tauri backend logs, unused sentry dep) and stub features exposed to end users.

| Severity | Finding | Evidence | Recommendation | Goals |
|----------|---------|----------|----------------|-------|
| Medium | Tauri desktop app has no tracing subscriber — all `tracing::` calls are silent | `apps/desktop/src-tauri/Cargo.toml`: `tracing-subscriber` and `tauri-plugin-log` absent; `lib.rs` has no subscriber init; ~88 `tracing::error!`/`warn!` call sites are no-ops | Add `tauri-plugin-log` and call `.plugin(tauri_plugin_log::Builder::new().build())` in `lib.rs` | G2, G3 |
| Medium | No crash reporting or panic hook installed | `Cargo.toml:504`: `sentry = "0.46.0"` declared but `grep 'use sentry\|sentry::' crates/ apps/'` returns zero; no `std::panic::set_hook` in any production entrypoint | Wire `sentry::init()` with opt-in, or install panic hook writing to OS log dir; remove unused dep | G2, G3 |
| Medium | TikTok and Instagram OAuth token exchange and upload are permanent stubs | `apps/desktop/src-tauri/src/publishing/tiktok.rs:92-98`; `instagram.rs:99-116`: both call `stub_complete_oauth`/`stub_upload` returning `ProviderError::Unsupported` | Remove from provider registry or mark "Coming Soon" in UI | G2, G3 |
| Medium | `eprintln!` used for data-integrity warning in `project.rs` production path | `crates/proto/src/project.rs:442-445`: `eprintln!("warning: {file}: emptied {dropped} malformed entr...")` in `Project::read()` | Replace with `tracing::warn!` and desktop event-channel notification | G2, G3 |
| Low | `social-server/main.rs` panics inside per-request helper `sign_public_artifact` | `crates/social-server/src/main.rs:954-956`: `Hmac::new_from_slice(..).unwrap_or_else(|_| panic!(...))` in request handler | Replace with `map_err` returning `ServerError`; assert key length at startup | G2 |
| Low | 113 Rust files exceed 500 LOC; 9 exceed 2,000 LOC | `render/src/timeline.rs`: 18,146 LOC; `core/src/edl/apply.rs`: 15,861 LOC; `social-server/src/main.rs`: 2,686 LOC mixing routes, DB, background jobs | Split `social-server/src/main.rs` into `routes.rs`, `db.rs`, `background.rs`; separate test modules in largest files | G1, G2 |
| Low | 163 raw `println!`/`eprintln!` calls in production Rust code bypass structured logging | `grep -rn 'eprintln!\|println!' crates/ (excluding cfg(test))` returns 163 hits; hotspots in `proto/project.rs`, `render/raw_stream*.rs` | Add `clippy::print_stdout` and `clippy::print_stderr` to workspace lints; replace with `tracing::` | G2 |
| Low | 22 `#[allow(clippy::unwrap_used)]` overrides in production code weaken workspace deny | `crates/index/src/lib.rs:288`: annotates public API function signature; 22 total production-code sites | Audit each site; for `index::run()` find specific `.expect()` and either prove infallible or return error | G2 |

---

## What's Already Solid

The following areas represent genuine strengths that should be preserved and built upon:

**Licensing foundation:** The Apache-2.0 root license is clean and unambiguous; all 20 first-party crates inherit it via `license.workspace = true`. `THIRD_PARTY_NOTICES.md` and `NOTICE` correctly attribute the vendored codex-rs fork (Apache-2.0) and bubblewrap (LGPLv2). The vendored `vendor/codex-rs/SOURCE` file is an exemplary attribution record. Audio assets are CC0 with a local LICENSE file.

**Community health files:** `README.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `MAINTAINERS.md`, `SECURITY.md`, `AGENTS.md`, `THIRD_PARTY_NOTICES.md`, PR template, and both bug-report and feature-request issue templates are all present. The `AGENTS.md` is a particularly well-written agent-native orientation document that exceeds typical contributor documentation quality.

**CI quality:** `ci.yml` runs format, clippy (`-D warnings`), tests, and doc checks on both `macos-latest` and `ubuntu-latest`. Workspace-level lints deny `unwrap_used`, `expect_used`, and ~30 other categories, catching violations at compile time. The Vite frontend build is gated on a frozen lockfile and protocol-binding drift detection. A separate weekly eval workflow exists.

**Sidecar packaging model:** The `yt-dlp` sidecar is correctly managed via Tauri's `externalBin` convention with a pinned version (`YT_DLP_VERSION = 2026.03.17`), a deterministic Makefile fetch from a pinned GitHub release URL, and support for all five target triples. This provides a tested, working template for bundling ffmpeg and montage-mcp-server.

**Auth implementation:** The ChatGPT OAuth flow is fully wired end-to-end in the GUI — the primary G3 prerequisite. Race-condition cancellation is correctly handled in `commands/auth.rs`. Token revocation on logout is server-side. Environment-variable override detection is surfaced in the UI. The `montage-auth` crate has good unit-test coverage for all four auth modes.

**Security fundamentals:** Path-traversal defenses are present in the Rust MCP tool layer (`AssetId::sidecar_relative_path()` rejects `..`, absolute prefixes, backslashes). The Tauri asset-protocol scope is dynamically narrowed to project-relative subdirectories. The macOS seatbelt sandbox uses `(deny default)` with a narrow writable subtree and passes its unit tests. Python MCP servers use list-form `subprocess.run` (no `shell=True`). API key validation strips whitespace and enforces the `sk-` prefix before writing to disk.

**Secret resolution architecture:** `crates/secrets/src/lib.rs` is well-designed — env-var override for CI, OS keychain for interactive use, no plaintext config files. Secondary secrets (HF_TOKEN, OPENROUTER_API_KEY, ANTHROPIC_API_KEY) are promoted to the process environment at startup so subprocesses inherit them. The `.gitignore` properly covers `.env`, `*.pem`, `*.key`, `*.p12`, and no real credentials were found anywhere in the tracked git history.

**Render test coverage:** The render crate has 17 integration test files covering loudnorm, transitions, speed ramps, output safety, GPU compositor, ASS captions, and audio volume — exercising the ffmpeg FFI boundary with real processes when available and skipping cleanly on headless CI. The `renderer_or_skip` pattern in GPU tests gracefully handles missing adapters.

**Lint rigor:** Production code in the hottest crates (`render/timeline.rs` at 18k LOC, `core/edl/apply.rs` at 15k LOC) is virtually free of raw unwraps before the `#[cfg(test)]` boundary. The Tauri command layer returns `Result` types that reach the frontend as human-readable strings rather than silent failures.
