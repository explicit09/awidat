# Risk Register — 2026-07-15

Output of a multi-agent audit (5 finder lenses × adversarial verification, Sonnet 5 workers).
27 raw findings → 25 confirmed, 2 refuted. 23 actionable risks below, ranked by severity;
2 entries are corrections to prior assumptions (bottom).

## Wave plan

| Wave | Theme | Risks |
|---|---|---|
| 0 | Land in-flight montage-eval WIP (both tool trees are being hand-edited simultaneously) | R22 |
| 1 | Finish codex-harness migration safely: port the 8 cross-tree dependencies + missing apply_edl logic, add montage_mcp apply_edl / picture-lock / dispatcher tests, THEN delete `crates/core/src/tools/` | R1, R5, R6, R7, R23 |
| 2 | Make CI trustworthy: run desktop Rust tests, chain the 16 dead frontend test scripts, tsc gate, de-flake DMG/sidecar release path | R2, R3, R17, R9 |
| 3 | Runtime hangs & data landmines: proxy ffmpeg timeout + real cancellation, stale `.pending` cleanup, export-poll timeout | R4, R11, R18 |
| 4 | Social stack: route tests for social-server, internal_tick coverage, delete dead sqlite_store + crates/tools stub | R8, R16, R14, R15 |
| 5 | Dependency hygiene: advisory-baseline triage, codex-rs upstream sync process, pin cargo-deny, revisit bans policy | R10, R12, R13, R19, R20, R21 |

Numbers refer to the entries below (ranked, not wave-ordered).

## Findings

### R1 — The two tool trees are not stale-duplicate copies but independently-diverged forks — 0/98 same-named files are byte-identical, and the 'new' montage_mcp tree still imports business logic from the 'legacy' tree it's supposed to supersede

**Severity:** critical · **Lens:** duplication · **Fix cost:** days · **Verification:** confirmed (high confidence)

**Evidence:** md5 comparison of all 98 same-named files in crates/core/src/tools/ vs crates/core/src/montage_mcp/tools/ shows 0 identical, 98 different. Magnitude varies wildly, e.g. apply_edl.rs is 1518 lines (legacy, at HEAD) vs 412 lines (montage_mcp, at HEAD) — legacy has normalize_edl_for_approval(), short_sha256(), and full ToolHandler/async_trait approval-key plumbing that montage_mcp's version simply does not have (confirmed via `grep -rn 'normalize_edl_for_approval|short_sha256' crates/core/src/` returning only the legacy file — not ported, not relocated). Worse: 8 files under crates/core/src/montage_mcp/tools/ directly call into crate::tools::* at runtime: apply_edl.rs:178 calls crate::tools::podcast_qc_report::is_podcast_project; render_preflight.rs:54 calls crate::tools::render_preflight::motion_scene_preflight_json; plan_motion_scene.rs:10 imports crate::tools::plan_motion_scene::{MotionScenePlanRequest, plan_motion_scene_request}; start_render.rs:79-80 calls crate::tools::podcast_qc_report::build_podcast_qc_report; podcast_apply_accepted_edits.rs, podcast_cleanup_candidates.rs, podcast_edit_proposal.rs all call crate::tools::find_dead_air / find_filler_words / find_false_starts.

**Blast radius:** Deleting crates/core/src/tools/ today (as the migration plan intends per the montage_mcp comments referencing 'step 5 of the codex-harness migration') would fail to compile crates/core/src/montage_mcp/tools/apply_edl.rs, render_preflight.rs, plan_motion_scene.rs, start_render.rs, podcast_qc_report.rs, podcast_apply_accepted_edits.rs, podcast_cleanup_candidates.rs, and podcast_edit_proposal.rs — i.e. it would break the tool that is actually served to the live agent (montage-mcp-server), not just remove dead code.

**Recommendation:** Before deleting crates/core/src/tools/, port find_dead_air/find_filler_words/find_false_starts/podcast_qc_report/render_preflight/plan_motion_scene logic into montage_mcp/tools/ or a shared non-tool module, then re-point the 8 cross-tree call sites. Also decide deliberately whether the dropped approval-hash/normalization logic in legacy apply_edl.rs was intentionally superseded by codex's own approval flow or silently lost.

### R2 — montage-desktop (Tauri backend) Rust tests never run in any CI workflow

**Severity:** high · **Lens:** ci-trust · **Fix cost:** hours · **Verification:** confirmed (high confidence)

**Evidence:** apps/desktop/src-tauri is a first-class workspace member (Cargo.toml:22 `"apps/desktop/src-tauri"`). .github/workflows/ci.yml:150 runs `cargo nextest run --workspace $VENDOR_EXCLUDES --exclude montage-desktop`, and the doctests step at ci.yml:156 also has `--exclude montage-desktop`. No other job (the 'desktop' frontend job, release.yml, evals.yml) runs `cargo test -p montage-desktop`. The crate contains 426 `#[test]`/`#[tokio::test]` functions across 50 files under apps/desktop/src-tauri/src/. The only way to run them is `make check-desktop-rust` (Makefile:46-49), which is documented in README.md:88 as a normal dev-lane command but is not invoked anywhere in .github/workflows/. Note: clippy and fmt for this crate ARE covered in CI (ci.yml:135-142 macOS clippy job explicitly includes `-p montage-desktop`; the top-level `cargo fmt --all -- --check` Format job covers it too) — it is specifically the test suite that is silently unchecked.

**Blast radius:** A contributor can break Tauri backend logic (IPC command handlers, render-queue state, social publishing glue, etc.) covered by 426 tests, get green CI on every job, merge to main, and only discover the regression when someone manually runs `make check-desktop-rust` or the packaged app misbehaves at runtime — closely mirrors the memory note about the desktop TS test-chain gap, but on the Rust side and for the full test suite rather than a subset of scripts.

**Recommendation:** Add a `cargo test -p montage-desktop` (or `make check-desktop-rust` minus the redundant fmt/clippy) step to either the existing 'Rust' matrix job or the 'Desktop frontend' job in ci.yml, gated appropriately for whatever sidecar/display stubs it needs (the job already runs `make desktop-sidecar-check-stubs`).

### R3 — 16 of 57 desktop frontend test:* scripts are dead code from CI's perspective — never chained into `pnpm test`

**Severity:** high · **Lens:** ci-trust · **Fix cost:** hours · **Verification:** confirmed (high confidence)

**Evidence:** apps/desktop/package.json defines 57 `test:*` scripts. Programmatically diffing the `test` script's chain (`npm run test:X && ...`, including the `test:campaign` aggregate's own sub-chain) against all defined `test:*` keys shows these 16 are never referenced, directly or transitively: test:agents-md-editor, test:ai-disclosure, test:center-mode, test:evidence-drill-downs, test:feedback-log, test:focus-controller, test:indexer-overlay, test:learned-patterns, test:perf-budget, test:perf-full, test:proposal-history, test:publishing-settings, test:rationale-display, test:render-queue-upload, test:skills, test:timeline-proposal-focus. These are not stubs — e.g. tests/render-queue-upload.test.ts is 649 lines, tests/skills-store.test.ts is 431 lines, tests/proposal-history.test.ts is 382 lines, tests/focus-controller.test.ts is 311 lines (verified by `wc -l`). CI's only desktop-test invocation is `pnpm --dir apps/desktop test` at ci.yml:203, which runs exactly the chained `test` script and nothing else.

**Blast radius:** Regressions in render-queue upload flow, skills store, focus controller, proposal history, publishing settings, and AI-disclosure logic can land on main with a fully green CI desktop job. This directly reproduces and extends the known 'desktop TS tests defined but not chained' gap noted from prior sessions — the gap is larger (16 scripts) than previously scoped, and includes both perf-budget guards and safety-relevant flows (ai-disclosure, publishing-settings).

**Recommendation:** Chain the 16 orphaned scripts into the `test` script (or a `test:full` superset that CI calls instead of `test`), and add a CI meta-check (e.g. a small Node script asserting every `test:*` key appears in the `test` script body) to prevent future additions from silently going uncovered.

### R4 — Agent-facing generate_proxy tool can hang indefinitely — no timeout on the ffmpeg child, cancellation token is a dead stub

**Severity:** high · **Lens:** correctness · **Fix cost:** hours · **Verification:** confirmed (high confidence)

**Evidence:** crates/core/src/tools/proxy_media.rs:165 and crates/core/src/montage_mcp/tools/proxy_media.rs:48 both call `montage_render::transcode_proxy(&asset_path, &pending_path, None, CancellationToken::new())` — a freshly-constructed token with no owner, so nothing can ever cancel it. Inside crates/render/src/ffmpeg.rs:786-900, transcode_proxy spawns ffmpeg with kill_on_drop but the only bound on the await is that same dead cancel token; there is no tokio::time::timeout wrapping the call. Compare with crates/render/src/job.rs:204-207, which documents `DEFAULT_JOB_TIMEOUT: Duration = Duration::from_secs(30*60)` for the desktop render-queue path (mirroring Codex's DEFAULT_AGENT_JOB_ITEM_TIMEOUT) — i.e. the codebase has an established convention for bounding ffmpeg jobs that these two agent tools do not follow. The rmcp tool_handler dispatch in crates/core/src/montage_mcp/mod.rs:2695 calls into tool `run()`/`handle()` directly with no outer timeout either.

**Blast radius:** Any agent turn that calls generate_proxy (either tool tree) on a problematic source file (corrupt container, exotic codec ffmpeg stalls decoding, or a source on a slow/disconnected network volume) blocks that tool call — and the whole agent turn — forever with no way for the harness to recover other than a hard process kill. Same dead-CancellationToken::new() pattern recurs in crates/core/src/tools/analyze_sync.rs:95,135, montage_mcp/tools/analyze_sync.rs:51,85, find_black_frames.rs (both trees), and verify_render.rs (both trees), so the same hang risk applies to waveform analysis, black-frame detection, and render verification tool calls.

**Recommendation:** Wrap these ffmpeg-shelling tool calls in `tokio::time::timeout(DURATION, ...)` (reuse render::job's DEFAULT_JOB_TIMEOUT or a smaller tool-appropriate bound) and map elapsed timeouts to a RespondToModel error with cleanup of any partial `.pending` file, so a stuck subprocess surfaces as a tool failure instead of a silent hang.

### R5 — apply_edl's approval-and-hook pipeline has zero integration-test coverage on the montage_mcp (production) path

**Severity:** high · **Lens:** duplication · **Fix cost:** days · **Verification:** confirmed (high confidence)

**Evidence:** crates/core/tests/editorial_workflow.rs (and 10 other files) exercise ToolRegistry + ApplyEdlTool from crate::tools:: exclusively (`use montage_core::tools::{apply_edl::ApplyEdlTool, ...}`, `use montage_core::{ToolContext, ToolHandler, ToolInvocation, ToolRegistry}`). grep across crates/core/tests/ for any file exercising montage_mcp::tools::apply_edl::run finds none — the 6 tests referencing montage_mcp (mcp_generated_media_cost.rs, plan_delivery_export.rs, plan_sound_design.rs, short_form_review.rs, plan_transition.rs, plan_split_edit.rs) cover other tools, not apply_edl.

**Blast radius:** The only tool actually served in production (montage-mcp-server binary, confirmed via grep — no binary anywhere constructs a ToolRegistry from crate::tools, only 6 test files do) has its most load-bearing operation validated solely against a code path (ToolRegistry/ToolHandler) that has no runtime entry point. A regression in montage_mcp::tools::apply_edl::run's dry_run/commit/hook logic could ship with all tests green.

**Recommendation:** Add an integration test exercising montage_mcp::tools::apply_edl::run directly (mirroring editorial_workflow.rs's scenario) before further shrinking or deleting the legacy ApplyEdlTool that currently provides the only coverage.

### R6 — montage_mcp apply_edl — the live MCP-facing mutating tool — has zero direct tests of its own wrapper logic (picture-lock gate, asset-existence check, hooks)

**Severity:** high · **Lens:** test-gaps · **Fix cost:** days · **Verification:** confirmed (high confidence)

**Evidence:** crates/core/src/montage_mcp/tools/apply_edl.rs (420 lines) contains no #[test]/#[tokio::test] and is not imported by any file under crates/core/tests/ (grep for `montage_mcp::tools::apply_edl` and `apply_edl::run(` across crates/core/tests/*.rs returns nothing). Its own doc comment calls it 'the load-bearing mutating tool... ported from crates/core/src/tools/apply_edl.rs to the in-process MCP server in step 5 of the codex-harness migration' (lines 1-7). By contrast the legacy crates/core/src/tools/apply_edl.rs has 20 inline #[test]s plus full integration coverage in crates/core/tests/editorial_workflow.rs (dry-run non-persistence, anchor-miss safety, rollback-on-partial-failure — see editorial_workflow.rs:297,333,361). The two implementations share the underlying edl_apply/AnchorContext/Project::read+write primitives (both call `edl_apply(&project.timeline, &envelope, &anchor_ctx)`), so core apply/rollback correctness is inherited and covered transitively. What is NOT covered is code unique to the montage_mcp wrapper: the picture_lock::check_envelope call (apply_edl.rs:51), the InsertClip asset-existence pre-check (lines 48-57), and the pre_apply_edl/post_apply_edl hook invocations (lines 62-66, 124-134) — none of these paths are exercised by any test that actually calls this file's `run()`.

**Blast radius:** Since the codex-harness migration plan is to delete the legacy tools tree, this file is becoming the sole implementation of the single most destructive operation in the product (mutates the project timeline on disk). A regression in the picture-lock check (e.g. an op wrongly classified as non-mutating slipping past a locked project) or the asset-existence guard would silently corrupt or desync a user's edit with no test to catch it before merge.

**Recommendation:** Port editorial_workflow.rs-style integration tests (dry-run non-persistence, anchor-miss handling, rollback) plus new tests specific to this wrapper: picture-lock-blocks-mutating-op, picture-lock-allows-sound/color-op, InsertClip-missing-asset-rejected, pre/post hook fire-and-forget behavior on hook failure.

### R7 — picture_lock::op_mutates_picture — the sole gate preventing post-lock picture corruption — has only 2 tests against an 81-variant, allowlist-inverted match

**Severity:** high · **Lens:** test-gaps · **Fix cost:** hours · **Verification:** confirmed (high confidence)

**Evidence:** crates/core/src/picture_lock.rs:76-133 hand-classifies each EdlOp variant as lock-exempt (~35 explicit arms: SetVolume, SetColorCorrection, InsertTitle, SetEffect, InsertTrack, DeleteTrack, etc.) with a catch-all `_ => true` (blocked) default. crates/core/src/edl/op.rs defines 81 EdlOp variants total (`grep -c '^    [A-Z][a-zA-Z]* {' op.rs` = 81). The test module (picture_lock.rs:163-189) contains exactly 2 tests: one 'allows_when_unlocked' and one 'blocks_picture_ops_when_locked' (both using Split Clip as the sole probed op). None of the ~35 explicitly-allowed variants (e.g. SetEffect, InsertTrack, DeleteTrack — all metadata/finishing ops asserted safe under lock) are individually tested to confirm they're actually let through, and no test exists at the integration level (crates/core/tests/) exercising picture_lock at all (grep for picture_lock/PictureLock across crates/core/tests/*.rs returns zero hits).

**Blast radius:** A miscategorized op (new EdlOp variant added to the allow-list that actually restructures picture, or an op omitted from the allow-list that should be exempt) would either let a locked cut silently reopen — defeating the entire picture-lock guarantee real post houses rely on — or spuriously block legitimate sound/color/graphics work after lock, with no test catching either direction.

**Recommendation:** Add a table-driven test that asserts every explicitly-listed 'safe' variant in op_mutates_picture returns false and spot-checks representative unlisted variants (InsertClip, DeleteClip, TrimClip, MoveClip, SetSpeed) return true, so future additions to the match arms are forced to declare intent explicitly rather than relying on the wildcard by accident.

### R8 — social-server internal_tick_handler — the worker route that claims and fires scheduled social publishes — has no route/handler-level tests

**Severity:** high · **Lens:** test-gaps · **Fix cost:** days · **Verification:** confirmed (high confidence)

**Evidence:** crates/social-server/src/main.rs:1017 defines internal_tick_handler (the /internal/tick POST route, main.rs:375), which gates on bearer auth, checks SOCIAL_FIRING_ENABLED, resolves the AEAD key, enforces a 100/day YouTube quota (lines 1049-1053), calls store.claim_due_publish_jobs, then branches per-provider (YouTube/TikTok/etc.) to build a live upload adapter and execute the claimed job. The file's only test module (main.rs:2215-2825, 21 tests) covers narrow helpers only — bearer_auth and HMAC artifact-signature verification (confirmed by reading the full test list: bearer_auth_accepts_matching_secret, bearer_auth_rejects_wrong_secret, public_artifact_signature_rejects_tampering, etc.). No test in the file or elsewhere in crates/social-server drives the handler itself: `grep -rln 'tower::ServiceExt|oneshot|TestServer' crates/social-server` finds no matches, and crates/social-server has no tests/ integration directory at all. The underlying store method (claim_due_publish_jobs) and quota logic ARE unit-tested via mocks in crates/social/src/upload_service.rs and crates/social/tests/pg_store.rs, but the HTTP-layer orchestration — quota-gate wiring, per-provider dispatch, error/restore-on-quota-block path (restore_youtube_quota_blocked_job) — is exercised nowhere.

**Blast radius:** This is the always-on server-side worker tick that the project's own architecture notes require for correctness ('scheduled posts MUST fire server-side'). A regression in the quota-restore logic, provider dispatch branching, or auth gate at the HTTP layer would silently drop, duplicate, or misfire scheduled social posts to live platforms with no automated signal before it reaches production.

**Recommendation:** Add an axum route-level test harness (tower::ServiceExt::oneshot against the Router) covering: unauthorized request rejected, SOCIAL_FIRING_ENABLED=false no-ops, quota-exhausted job left Scheduled and restored, and a happy-path claim+dispatch using a mock store/adapter.

### R9 — Release DMG build depends on a rolling GitHub Release cache (codex-sidecar-cache) that can silently fall back to a 19-57 min inline rebuild, and 3x retry masks intermittent notarization/build flakiness without alerting

**Severity:** medium · **Lens:** ci-trust · **Fix cost:** hours · **Verification:** confirmed (high confidence)

**Evidence:** release.yml 'Fetch prebuilt codex sidecar' step (around the `gh release download codex-sidecar-cache --pattern "$asset"` line) falls back silently (`else echo ... build from source`) if no cached asset matches the content-hash key. The prebuild is produced by codex-sidecar.yml, a separate workflow triggered on push to main touching vendor/codex-rs or Cargo.lock — if that workflow fails or hasn't completed yet when a release tag is pushed, the release job silently takes the slow path instead of failing loudly. Separately, 'Build signed DMG' retries `tauri build` 3 times with a 30s sleep on any failure (release.yml, `for attempt in 1 2 3`), which will mask a flaky codesign/notarization issue as a transient one without surfacing it, per prior known 'codesign timestamp-server flake' comment in the same file.

**Blast radius:** A release can take 2-3x longer than expected with no clear signal why (silent cache miss), and repeated transient notarization flakes are absorbed by retries rather than tracked, so a systemic Apple-side or environment issue could go unnoticed until it fails all 3 attempts.

**Recommendation:** Emit a workflow annotation/warning when the prebuilt sidecar cache misses (so it's visible in the run summary, not just build-log text), and add a metric/log line counting retry attempts across releases to catch a rising flake rate before it becomes a full failure.

### R10 — cargo-deny advisory baseline carries 26 ignored RUSTSEC IDs, several explicitly framed as permanent/unresolved upstream blockers

**Severity:** medium · **Lens:** ci-trust · **Fix cost:** days · **Verification:** confirmed (high confidence)

**Evidence:** deny.toml `[advisories] ignore = [...]` lists 26 RUSTSEC IDs (RUSTSEC-2024-0320, -0370, -0388, -0411 through -0420 [GTK3/Tauri, 7 entries], -0436, RUSTSEC-2025-0057, -0075, -0080, -0081, -0098, -0100, -0141, RUSTSEC-2026-0097, -0118, -0119, -0173, -0189). Several reasons explicitly state 'no safe upgrade in current graph, tracked for upstream update' (Tauri urlpattern chain) or 'tracked until Tauri dependency path removes it' (7 GTK3 entries) — i.e. cargo-deny will keep silently passing over these regardless of severity until someone manually revisits deny.toml, and CI's 'Security hygiene' job (ci.yml `cargo deny check`) cannot distinguish 'newly acceptable' from 'stale forever-ignored'.

**Blast radius:** A future advisory affecting one of these already-ignored crates (or a severity escalation on an existing one) would not fail CI, since the ID is unconditionally ignored rather than pinned to a version/date. This is a known and load-bearing gate per prior CI-checks memory, but the ignore list has grown large enough that it functions as a standing exemption rather than a curated one.

**Recommendation:** Periodically audit deny.toml's ignore list against current RUSTSEC status (advisories can be withdrawn, or the transitive dep may have since been removed), and consider cargo-deny's per-advisory expiry mechanism if available, so ignores don't silently outlive their justification.

### R11 — Stale .pending proxy files are never cleaned up by the orphan-pruning pass, and proxy_status reports them as 'pending' forever with no staleness signal

**Severity:** medium · **Lens:** correctness · **Fix cost:** hours · **Verification:** confirmed (high confidence)

**Evidence:** crates/core/src/proxy.rs:78-91 — proxy_status_for treats any existing `<proxy>.mp4.pending` file as ProxyStatus::Pending with no check of the pending file's mtime/age. apps/desktop/src-tauri/src/commands/transcode.rs:164-201, cleanup_orphaned_proxies_in only deletes entries whose ProxyCacheStatus is Orphan; the test at transcode.rs:997-1020 (`cleanup_deletes_orphan_proxies_but_keeps_stale_and_pending`) explicitly documents that stale + pending files are 'not touched'. crates/core/src/media_intelligence.rs:171 maps ProxyStatus::Pending to MediaIntelligenceLayerStatus::Processing for the UI.

**Blast radius:** If the desktop app is killed (crash, force-quit, OS reboot) mid-transcode, the `.pending` file is left on disk. The desktop's own next-launch transcode path recovers fine (reserve_proxy_transcode is in-memory and transcode_one_reserved always overwrites the pending file — verified at transcode.rs:580-596), but the agent-facing proxy_status tool (crates/core/src/tools/proxy_media.rs, montage_mcp/tools/proxy_media.rs) and the media-intelligence readiness report have no equivalent recovery: they will keep reporting 'pending'/'processing' to the agent indefinitely for an asset that no process is actually working on, unless something else independently calls generate_proxy again. An agent that trusts 'processing, wait and re-check' guidance could poll this status forever waiting for a transcode that isn't running.

**Recommendation:** Add an age check to proxy_status_for (or a variant used by the agent tools): if the `.pending` file's mtime exceeds a reasonable transcode-duration bound (e.g. 10-15 min) and no in-memory job owns it, report Missing (or a new Stale-pending state) rather than Pending, so the agent tool layer can retrigger generate_proxy instead of waiting on a phantom job.

### R12 — 26 RUSTSEC advisories permanently suppressed in deny.toml, mostly with no remediation trigger

**Severity:** medium · **Lens:** deps-security · **Fix cost:** days · **Verification:** confirmed (medium confidence)

**Evidence:** deny.toml lines 18-46 list 26 `[[advisories.ignore]]` entries. 5 are tagged 'unmaintained ... tracked as follow-up dependency cleanup' (proc-macro-error RUSTSEC-2024-0370, derivative 2024-0388, paste 2024-0436, fxhash 2025-0057, proc-macro-error2 2026-0173) with no owner, date, or issue link. 8 are GTK3-binding advisories (2024-0411..0420) blanket-ignored 'until Tauri dependency path removes it' and 5 are Tauri urlpattern advisories (2025-0075/0080/0081/0098/0100) ignored as 'no safe upgrade in current graph.' None of these have a tracking issue reference, expiry date, or automated re-check — `cargo deny check` will pass silently forever even if upstream ships a fix. Last touched 2026-07-01 (commit c96857ca 'refresh advisory gate baseline'), i.e. a bulk baseline reset rather than per-advisory triage.

**Blast radius:** Any of the 5 'unmaintained' crates or the Tauri/GTK3 chain could have a newly-disclosed real vulnerability added to the same RUSTSEC id's advisory text later, or a new advisory could land in a crate already on the ignore list's transitive path — cargo-deny would not surface it because the id is preemptively suppressed only for the current known advisory, but the sheer count (26) makes it easy for a genuinely new, more severe entry to get lost in the noise when triaging `cargo deny check` diffs.

**Recommendation:** Add expiry dates or tracking-issue links to each ignore entry (cargo-deny supports this pattern informally via comments; formalize with a linked issue number), and split 'unmaintained, no fix available' entries from 'fix available but not yet applied' entries so the backlog is actionable instead of a single 26-line wall.

### R13 — vendor/codex-rs fork has no automated upstream security-patch tracking

**Severity:** medium · **Lens:** deps-security · **Fix cost:** week-plus · **Verification:** confirmed (high confidence)

**Evidence:** vendor/codex-rs/SOURCE states: 'Forked at commit 8a94430b... Date: 2026-05-25 ... We own this fork. Edits are normal code changes; we do not track upstream automatically. To pull a specific upstream change later, cherry-pick from a local clone of openai/codex.' Fork is currently ~7 weeks behind upstream openai/codex (forked 2026-05-25, today 2026-07-15) with 11 documented manual patches (approval-policy override, OAuth client id substitution, websocket metadata shape, montage TUI panel, etc.) that must be re-applied by hand on every refresh per the file's own instructions.

**Blast radius:** If openai/codex ships a security fix (e.g. to sandbox escape, apply-patch path handling, or auth token handling) upstream, there is no mechanism in this repo that would surface it — it depends entirely on someone manually noticing and cherry-picking. The 11 documented hand-reapplied patches also mean every refresh is a manual, error-prone merge with no test asserting the patches survived.

**Recommendation:** Add a periodic (e.g. monthly) check against openai/codex's release/security advisories, or a CI job that diffs vendor/codex-rs/SOURCE's pinned commit against upstream HEAD and flags when it's more than N weeks stale. Consider converting the 11 hand-reapplied patches into a patch-file/git-cherry-pick script so refreshes are reproducible rather than manual re-typing.

### R14 — crates/tools workspace member is a dead Week-1 stub with zero dependents, superseded twice over

**Severity:** medium · **Lens:** duplication · **Fix cost:** hours · **Verification:** confirmed (high confidence)

**Evidence:** crates/tools/src/lib.rs is 20 lines, last touched 2026-06-07 (`git log -1 --format="%ai" -- crates/tools/`), contains only `pub const PLANNED_V1_TOOL_COUNT: usize = 12;` with a comment 'Week 1 stub. Real tools ... land in Week 4'. `grep -rl "montage-tools\b" --include=Cargo.toml .` returns only the workspace root Cargo.toml and crates/tools/Cargo.toml itself — no crate anywhere depends on it. It predates both crates/core/src/tools (98 tools) and crates/core/src/montage_mcp/tools (115 tools).

**Blast radius:** None if deleted — pure dead weight in the workspace member list, adds a compile unit to every full-workspace build for zero functional value.

**Recommendation:** Remove crates/tools entirely (both the member entry in the root Cargo.toml and the directory) — a trivial, safe cleanup with no compile-time dependents to fix.

### R15 — crates/social's sqlite_store.rs (1315 lines) is dead in production, kept alive only as a test fixture for the pre-Phase-5 in-process path

**Severity:** medium · **Lens:** duplication · **Fix cost:** hours · **Verification:** confirmed (high confidence)

**Evidence:** `grep -rln "sqlite_store::" crates/ apps/` returns only crates/social/src/api.rs (used exclusively inside #[cfg(test)] blocks at lines 2056/2060/2101/2168 constructing `SqliteSocialStore::new_in_memory()`) and crates/social/tests/pipeline_e2e.rs. Per apps/desktop/src-tauri/src/social_client.rs:1-4 doc comment: 'Phase 5 moves the desktop's social-publishing path off the in-process SocialApi + local SQLite store and onto the server.' No production binary (desktop or social-server) constructs SqliteSocialStore.

**Blast radius:** None functionally if removed from the non-test build — it is already unreachable from any running binary. Its only cost today is 1315 lines of maintained-but-unused code plus the SocialStore trait surface it has to keep satisfying.

**Recommendation:** Either gate sqlite_store.rs behind #[cfg(test)] explicitly (it's already only test-consumed) or delete it and inline an equivalent in-memory test double, removing the illusion that it's a supported storage backend.

### R16 — social-server has zero integration/route tests anywhere in the crate despite ~20 HTTP routes including OAuth callback and artifact upload/serve

**Severity:** medium · **Lens:** test-gaps · **Fix cost:** days · **Verification:** confirmed (high confidence)

**Evidence:** crates/social-server/src/main.rs:309-380 registers ~20 routes (oauth_begin_handler, oauth_callback_handler, artifacts_upload_url_handler, public_artifact_handler, internal_poll_processing_handler, internal_refresh_tokens_handler, plus the user_routes:: handlers for /social/accounts, /social/jobs, etc.). `find crates/social-server -type d -name tests` returns nothing, and `ls crates/social-server` shows no tests/ directory — the crate has Cargo.toml, Dockerfile, fly.toml, run-local.sh, src only. All 53 tests across the crate's 6 source files (main.rs:21, user_routes.rs:13, artifact_source.rs:9, supabase_jwt.rs:5, token_resolver.rs:3, token_refresher.rs:2) are unit tests of pure functions/helpers, not handler or route-wiring tests.

**Blast radius:** Route-level regressions (wrong status code, missing auth check on a newly added route, JSON shape drift breaking the desktop client) would only surface in production or manual QA, not CI.

**Recommendation:** Stand up a minimal axum test harness with an in-memory/sqlite-backed SharedState and cover the OAuth callback and artifact upload/serve happy+auth-failure paths first, since those are the routes with external-facing security surface (signature verification, token exchange).

### R17 — Desktop CI job builds/tests the frontend but never runs `pnpm --dir apps/desktop exec tsc --noEmit` as a distinct gate — TypeScript correctness is only as strong as what `vite build` and the ad-hoc per-test `tsc` invocations happen to cover

**Severity:** low · **Lens:** ci-trust · **Fix cost:** hours · **Verification:** confirmed (high confidence)

**Evidence:** ci.yml 'desktop' job runs `pnpm --dir apps/desktop build` (which runs `tsc && vite build` per the `build` script in package.json) and then `pnpm --dir apps/desktop test`. Several individual test scripts invoke `tsc` themselves with narrow file lists and custom flags (e.g. `test:animation` compiles only 2 files with `--skipLibCheck`; `test:play-segments` compiles a specific file list to CommonJS) rather than the project-wide tsconfig, so type-checking coverage for test-only code paths is inconsistent and scattered rather than a single authoritative gate.

**Blast radius:** Type errors confined to files not touched by the main `build` tsc pass and not part of the narrow per-test tsc invocations (e.g. dead code, or new test files using different APIs) can go undetected by CI.

**Recommendation:** Add a single `tsc --noEmit -p tsconfig.json` (or equivalent full-project check) as its own CI step, independent of the build and ad-hoc per-test compiles.

### R18 — useExportJob polling loop has no client-side timeout — a stuck render job status leaves the Export UI polling forever

**Severity:** low · **Lens:** correctness · **Fix cost:** hours · **Verification:** confirmed (high confidence)

**Evidence:** apps/desktop/src/app/useExportJob.ts:94-174 — the setInterval poll loop only clears on 'done', 'failed', 'cancelled', or an invoke() throw; 'queued'/'running' just re-upsert progress and continue polling every 500ms (POLL_INTERVAL_MS, line 37) with no elapsed-time cap. It does rely on the backend's JobManager DEFAULT_JOB_TIMEOUT (30 min, crates/render/src/job.rs:207) to eventually flip the job to Failed, but that backend timeout only applies to jobs whose child process is being tracked by JobManager's watchdog — if `render_jobs.status()` itself throws or if a code path leaves the watch-channel status wedged at Running without the watchdog task attached, the frontend has no independent ceiling and will spin at 500ms/poll indefinitely.

**Blast radius:** Worst case is a UI progress bar that never resolves and never tells the user to give up/retry; not a data-loss or deadlock risk since it's frontend polling, but it directly matches the 'preview hangs forever' pattern already logged in project memory for proxy schema bumps.

**Recommendation:** Add a max wall-clock bound in useExportJob (e.g. mirror the backend's 30-min DEFAULT_JOB_TIMEOUT plus margin) that force-clears the interval and surfaces a 'render timed out — check backend' terminal state if no terminal status arrives in time.

### R19 — cargo-deny installed unpinned in CI security job

**Severity:** low · **Lens:** deps-security · **Fix cost:** hours · **Verification:** confirmed (high confidence)

**Evidence:** .github/workflows/ci.yml lines 42-45: `run: cargo install cargo-deny --locked` has no version pin (unlike other tool installs in the repo that pin exact versions, e.g. the gitleaks image is pinned to `v8.24.3` two lines later in the same job).

**Blast radius:** A new cargo-deny major version could change default policy behavior (e.g. stricter or looser advisory/license defaults) and silently pass or fail CI differently across runs without a corresponding repo change, making the security gate non-reproducible.

**Recommendation:** Pin cargo-deny to an exact version (`cargo install cargo-deny --version X.Y.Z --locked`) matching the gitleaks pinning convention already used in the same job.

### R20 — RUSTSEC-2026-0189 (rmcp Streamable HTTP server transport) suppression reasoning isn't verified against actual feature flags in the ignore comment

**Severity:** low · **Lens:** deps-security · **Fix cost:** hours · **Verification:** confirmed (high confidence)

**Evidence:** deny.toml line 45 ignores RUSTSEC-2026-0189 with reason 'rmcp 0.15 is deliberately pinned to match vendored Codex; advisory affects Streamable HTTP server transport...'. Root Cargo.toml line 361 sets `rmcp = { version = "0.15.0", default-features = false }`; crates/core, crates/mcp, and all vendor/codex-rs consumers (mcp-server, tui, codex-mcp, rmcp-client) enable only `server`, `client`, `macros`, `schemars`, `base64`, `transport-io`, `transport-child-process` features — none enable a streamable-HTTP-server transport feature, so the vulnerable code path does not appear to compile into this workspace at all today.

**Blast radius:** Low as currently configured (the affected transport isn't reachable), but the ignore-reason text reads as if the vulnerable surface is present and merely tracked for a future migration, rather than stating it's not compiled in. If a future PR adds an HTTP-transport feature to any rmcp dependency, this pre-existing blanket ignore would silently continue suppressing the advisory with a now-inaccurate justification.

**Recommendation:** Update the ignore reason to state explicitly that no crate currently enables the streamable-HTTP-server rmcp feature, and add a lightweight check (grep in CI, or a `cargo tree -e features` assertion) that fails if that feature is ever turned on while the advisory remains suppressed.

### R21 — cargo-deny bans.multiple-versions is globally set to 'allow' with no revisit trigger

**Severity:** low · **Lens:** deps-security · **Fix cost:** week-plus · **Verification:** confirmed (high confidence)

**Evidence:** deny.toml lines 63-67: `multiple-versions = "allow"` with comment 'Duplicate dependency cleanup is noisy in the vendored Codex/Tauri graph and should be handled as a separate dependency-consolidation effort.' Cargo.lock confirms real duplication in security-relevant crates, e.g. `constant_time_eq` appears at both 0.1.5 and 0.3.1 (Cargo.lock lines 4562-4568, 18608), meaning two different major versions of a timing-safety-critical crate coexist in the dependency graph.

**Blast radius:** Duplicate major versions of security-sensitive crates (constant-time comparison, crypto primitives) mean a vetted/patched version in one part of the graph doesn't guarantee the same guarantee elsewhere; cargo-deny's ban gate is fully disabled workspace-wide so this can silently grow.

**Recommendation:** Scope multiple-versions=allow to only the vendor/codex-rs subtree (cargo-deny supports per-crate exceptions) rather than the whole workspace, so first-party crates (secrets, auth, social-server) get duplicate-version detection.

### R22 — Both tool-tree directories are being actively hand-edited in the uncommitted working tree right now (git status shows both apply_edl.rs and load_skill.rs modified in both trees simultaneously), increasing risk that this audit's file-level diffs go stale immediately

**Severity:** low · **Lens:** duplication · **Fix cost:** hours · **Verification:** confirmed (high confidence)

**Evidence:** `git status --short` at session start shows M crates/core/src/tools/apply_edl.rs, M crates/core/src/montage_mcp/tools/apply_edl.rs, M crates/core/src/tools/load_skill.rs, M crates/core/src/montage_mcp/tools/load_skill.rs all modified uncommitted, plus untracked crates/core/src/editorial_tags.rs, crates/core/src/picture_lock.rs, crates/core/src/skill_session.rs, and crates/core/src/montage_mcp/tools/set_picture_lock.rs. `git diff --stat` shows the legacy apply_edl.rs is shrinking (152 deletions, minimal additions) in this same uncommitted change while montage_mcp's grows by 20 lines — consistent with an in-progress convergence/migration edit, not settled state.

**Blast radius:** Any 'delete legacy tree' decision made from a stale read of current diffs could be wrong within the same session; the new editorial_tags.rs/picture_lock.rs/skill_session.rs modules are being wired into both trees as shared helpers, suggesting the migration strategy may be shifting from 'port everything to montage_mcp' to 'extract shared modules used by both.'

**Recommendation:** Re-run this duplication analysis after the current uncommitted work is committed, since the picture (0/98 identical, 8 cross-tree production dependencies) may shift as editorial_tags.rs/picture_lock.rs/skill_session.rs absorb more shared logic.

### R23 — montage_mcp dispatcher (montage_mcp/mod.rs, 2696 lines) has zero direct tests; apply_edl's dispatcher wiring specifically is unverified

**Severity:** low · **Lens:** test-gaps · **Fix cost:** hours · **Verification:** confirmed (high confidence)

**Evidence:** crates/core/src/montage_mcp/mod.rs is 2696 lines with 0 #[test]/#[cfg(test)] occurrences. Coverage of the dispatcher is indirect, via 12 integration test files under crates/core/tests/ that invoke individual tools by name (list_assets_bin.rs, continuity_integration.rs, plan_split_edit.rs, plan_transition.rs, etc.) — but apply_edl is not among the tools those tests route through (confirmed: no test file calls the dispatcher with tool name 'apply_edl' and inspects the montage_mcp result), so the dispatcher's apply_edl registration/wiring path specifically is untested end-to-end.

**Blast radius:** Low standalone risk since this is a thin routing layer, but it compounds finding #1 — a wiring bug (wrong schema, misrouted args) in how the dispatcher hands off to apply_edl would not be caught by any existing test.

**Recommendation:** Once the apply_edl wrapper gets direct tests (finding #1), route at least one of them through the full montage_mcp dispatcher rather than calling apply_edl::run directly, closing both gaps with one test.

### R24 — Config::overlay silently discards ALL editorial hooks config (pre/post_apply_edl are dead in production)

**Severity:** high · **Lens:** correctness · **Fix cost:** hours · **Verification:** confirmed (found during Wave 1b test-writing, verified at crates/config/src/lib.rs:284-287)

**Evidence:** `Config::overlay` unconditionally returns `hooks: HooksConfig::default()`, discarding both the global and the project `[hooks]` section. `apply_edl::run` reaches hooks via `Config::load()` → `overlay()`, so `pre_apply_edl`/`post_apply_edl` configured in `.montage/config.toml` never fire. The Wave 1b hook-blocking test is committed but `#[ignore]`d pending this fix (crates/core/tests/montage_mcp_apply_edl.rs).

**Blast radius:** any project relying on a pre_apply_edl hook as a safety gate (lint, backup, validation) gets no protection and no error — the hook is silently skipped.

**Recommendation:** merge hooks in overlay (project overrides global per-field), un-ignore the hook test, add an overlay unit test asserting hooks survive.

## Status log

- 2026-07-15: Approval-hash decision (R1 prerequisite): legacy `approval_keys`/`normalize_edl_for_approval`/`short_sha256` served ONLY the legacy harness's session-approval cache (`crate::tool::approved_for_session`), which no binary reaches since the codex-engine cutover. Codex gates mutating tools pre-execution via MCP destructive_hint annotations. Verdict: SUPERSEDED — delete with the tree, do not port. Only delta is lost re-prompt memoization for identical retried EDLs (UX, not safety).
- 2026-07-16: WAVE 2 DONE — R2 (desktop Rust tests on both CI legs), R3 (14/16 dead suites chained + zustand localStorage bug fixed; perf-budget/perf-full remain, blocked on Playwright cache), R17 (tsc gate), R9 (sidecar-cache miss loud + evidence-preserving DMG retry; validates on next v* tag), R25 (stage-harness stale-compositor root cause + recapture-with-evidence; run 29463364029 fully green, recapture observed working at the exact historical bimodal SSIMs). Main CI green end-to-end.
- 2026-07-15: R1 DONE — legacy crates/core/src/tools/ deleted (104 files, ~47k lines); montage_mcp is the sole tool surface. R14 DONE — montage-tools stub removed. R24 DONE — hooks merge fixed, hook test live. Follow-up open: delete dead ToolRegistry/ToolHandler infra once lessons.rs drops ApprovalKey.
- 2026-07-15: R22 done (WIP landed, 095de43e). R1 unblocked (cross-tree deps ported, b830aea8; legacy-tree deletion pending R-approval-hash decision). R5/R6/R7/R23 done (6a5fa802). R24 added.

### R25 — stage-harness visual gate is nondeterministically flaky on CI (~50% of main pushes fail on a random case)

**Severity:** high · **Lens:** ci-trust · **Fix cost:** days · **Verification:** confirmed (observed live: run 29437335962 failed scene-basic t=1.0 SSIM=0.824; run 29460563860 failed scene-kinetic-text t=1.5 SSIM=0.819; each time the other 4 cases scored exactly 1.000000; upstream merge c4ed38e9 failed the same way pre-Wave-1)

**Evidence:** a different case fails per run with SSIM ~0.82 while all others are byte-perfect — a frame-capture race (screenshot lands on a mid-animation frame), not rendering divergence. Upstream PRs #98/#101 already added rVFC/paint gating; insufficient.

**Blast radius:** main CI red ~half the time regardless of the change being tested; the whole Desktop frontend job's verdict is noise, training everyone to ignore red CI — the root disease Wave 2 exists to cure.

**Recommendation:** make the capture deterministic (drive the animation clock manually instead of free-running: pause + set time, capture the presented frame), or as a stopgap re-seek-and-recapture once with both attempts' SSIM logged and screenshots uploaded so true regressions stay visible. Mitigated meanwhile by moving stage-harness to the end of the test chain (2026-07-15) so it cannot shadow deterministic suites.

## Corrections to prior beliefs (not risks)

- **montage_social crate dependency in apps/desktop/src-tauri is legitimate (DTO reuse), not leftover legacy wiring — refutes the 'still depends on legacy' framing from prior-session memory** — apps/desktop/src-tauri/src/social_client.rs imports montage_social::api::* and montage_social::model::* purely as shared serde DTOs for its HTTPS client to montage-social-server (doc comment: 'sends/receives the re-exported montage_social::api DTOs so client and server agree on exactly one serde shape'). apps/desktop/src-tauri/src/commands/social.rs uses SocialApi::providers(&registry) at line 45,
- **clip_anchor mcp wrapper vs core anchor.rs — checked, confirmed adequately covered (not a gap)** — crates/core/src/edl/anchor.rs: 16 #[test] (1061 lines). crates/core/src/montage_mcp/tools/clip_anchor.rs: 21 #[test] (657 lines) — both reasonably covered, noted here only to document this pairing was checked and ruled out, unlike apply_edl's analogous pairing which is a real gap (finding #1).
