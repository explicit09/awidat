# Phase 4: pg_cron scheduler + token-refresh sweep firing jobs through the montage-social service

> **BINDING:** Read `RECONCILIATION.md` first. Where this plan conflicts with it, RECONCILIATION wins. Key: domain stays sync (D1), server crate = `crates/social-server` (D2).


**Depends on phases:** [1, 2, 3]

## Prerequisites
- [USER] Supabase project must exist with the `pg_cron` and `pg_net` extensions enabled (pg_cron/pg_net are enable-per-project in Supabase) — created in Phase 1, confirmed enabled here.
- [USER] The montage-social Rust HTTP service must be deployed and reachable from Supabase (Fly.io/Railway/container per the spec's open question) with a stable base URL — Phase 1 deliverable; Phase 4 assumes it exists.
- [USER] Set a shared cron secret (e.g. CRON_INTERNAL_TOKEN) and the service base URL as Supabase Vault secrets / DB settings, so the migration references them by name rather than embedding literals.
- [USER] A real YouTube OAuth app (client_id/client_secret) with refresh-token issuance, and at least one connected test account (Phase 2 deliverable) to exercise refresh-on-fire; YouTube API project TOS audit NOT required for Phase 4 — private-mode firing is sufficient to demo.
- [WE-STAGE] The pg_cron schedule SQL + pg_net http_post glue (Step 5) as a reversible migration.
- [WE-STAGE] The /internal/cron/* service routes, shared-secret guard, new store query methods, and the backoff + token-refresh domain logic (Steps 1-4), all behind tests using local Postgres + a mocked provider so they can be staged and verified without any audit clearing.

## Plan

## Phase 4 — pg_cron scheduler + token-refresh sweep firing jobs through the service

### Context grounded in the actual code

The firing *primitives* already exist and are unit-tested in `crates/social/src`; Phase 4 does NOT reimplement them. What exists today:

- `crates/social/src/publish_service.rs::PublishService::claim_due_jobs` (line 167) → `store.claim_due_publish_jobs(now, limit)`.
- `crates/social/src/store.rs::SocialStore::claim_due_publish_jobs` (trait line 73). The in-memory impl (line 257) selects `status==Scheduled && scheduled_for<=now`, sorts by `scheduled_for`, applies `job.claim_for_upload(now)`; the SQLite impl is identical SQL (`crates/social/src/sqlite_store.rs` line 447, `WHERE status=? AND scheduled_for<=?`).
- `crates/social/src/job.rs::claim_for_upload` (line 112) sets `Uploading` and bumps `attempt_count` via `saturating_add(1)`.
- The worker execute/poll path: `crates/social/src/api.rs::execute_claimed_upload_job` (line 644) and `poll_upload_status` (line 678), delegating to `crates/social/src/upload_service.rs::UploadService::execute_claimed_job` (all merged safety guards: account-re-check at lines 67-88, cancel-race re-read at lines 122-136, privacy resolution at `api.rs` line 653) and `crates/social/src/upload_status.rs::UploadStatusService::poll_processing_job` (line 79).

What is genuinely MISSING and is this phase's work:

1. **No scheduler at all** — nothing periodically calls `claim_due_jobs` + `execute`. This phase adds the pg_cron minute-tick and the HTTP worker tick endpoint it calls.
2. **No bounded retry/backoff** — `crates/social/src/job.rs::fail` (line 154) is *immediately terminal* (`status=Failed`). There is no logic that, on a retryable provider error, checks `attempt_count` and either re-schedules with a backoff delay or goes terminal `Failed`. `retry()` (line 167) exists but is only wired to the *manual* `retry_job` route. This phase adds the automatic bounded-retry decision.
3. **No token-refresh sweep and no refresh-on-fire** — `crates/social/src/token.rs` is an XOR stub with no refresh; `access_token_expires_at` exists on `TokenSecret` (line 11) but nothing refreshes. Phase 2 is assumed to deliver the actual OAuth refresh capability (exchange against the provider using `client_secret`); this phase adds (a) the pg_cron sweep that finds near-expiry tokens and drives a refresh, and (b) the "ensure-fresh-token-before-fire" hook in the worker tick, plus the refresh-failure → `NeedsReauth` transition.

This phase therefore lands changes in three places: SQL/cron in the Supabase project (created in Phase 1), new HTTP worker-tick endpoints on the Rust service (scaffolded in Phase 1), and a small amount of new domain logic in `crates/social/src` (backoff decision + refresh-driver glue). Everything else is reused unchanged.

---

### Step 1 — Add the bounded-retry/backoff decision to the domain crate (pure logic, TDD)

**File:** `crates/social/src/job.rs`

Add a method on `PublishJob` that, given the current `attempt_count`, a `max_attempts` budget, a base backoff, and `now`, returns either a re-scheduled job (status back to `Scheduled`, `scheduled_for = now + backoff(attempt_count)`, errors cleared like `retry()` does) or a terminal `Fail`. Model it as a new method, e.g. `fn retry_with_backoff_or_fail(self, max_attempts, base_backoff_secs, normalized_error, raw_error_ref, now) -> (PublishJob, RetryOutcome)` where `RetryOutcome` is a small enum (`Requeued { next_scheduled_for } | Exhausted`). Reuse the existing `schedule`/`retry`/`fail` transitions internally rather than hand-rolling status writes. Backoff = exponential: `base * 2^(attempt_count-1)` capped at a ceiling const.

Why here: `attempt_count` is already incremented at claim time (`claim_for_upload`), so after a failed attempt the count reflects attempts-so-far; the decision is pure and belongs next to the FSM it mutates, keeping it unit-testable with no I/O.

**Verify:** add `#[cfg(test)]` cases in `job.rs` mirroring the existing style (see the existing `publish_job_schedule_claim_fail_and_retry_transitions` test, line 336): (a) attempt 1 of 3 → `Requeued` with `scheduled_for == now + base`; (b) attempt 2 → `scheduled_for == now + 2*base`; (c) attempt == max → `Exhausted`, `status==Failed`, `normalized_error` set; (d) backoff capped at ceiling. Run `cargo test -p montage-social job::`.

### Step 2 — Wire the backoff decision into the worker upload path

**File:** `crates/social/src/upload_service.rs`

Today the `NetworkOrServer` and (arguably) `MediaConstraintFailed` arms (lines 195-233) call `upload_in_progress.fail(...)` immediately. Introduce a notion of *retryable vs terminal* provider failure and, for retryable ones (`NetworkOrServer`, and provider-side transient/5xx surfaced by the real adapter from Phase 3), call the new `retry_with_backoff_or_fail` from Step 1 instead of unconditional `fail`. Keep `MediaConstraintFailed` terminal (a 4xx media rejection won't fix itself). Keep `RequiresAction`/`MissingUploadToken` mapping to `RequiresAction` unchanged. Append a `RetryQueued` event (the enum variant already exists, `PublishJobEventType::RetryQueued`) when requeued, and `Failed` when exhausted, reusing the existing `append_event` helper (line 243).

**Per G5 — keep the tested domain struct stable.** Do NOT add `max_attempts`/`base_backoff_secs` fields to `ExecuteUploadInput` (upload_service.rs:9-20) or `ExecuteUploadRequest` (api.rs:381) — that signature change ripples to every caller including the desktop mock path. Instead, the backoff **policy** (`max_attempts`, `base_backoff_secs`, ceiling) lives as **server-worker config in `crates/social-server`**, and the worker passes the computed decision down by calling the new `PublishJob::retry_with_backoff_or_fail(...)` (Step 1) from the retryable error arm. If a parameter genuinely must reach `upload_service`, prefer a single small `RetryPolicy` value with a `Default` impl so existing call sites compile unchanged. Keep the domain structs and their tests untouched.

Why: this preserves every existing guard (the account re-check and cancel-race logic stay exactly as-is; we only change which terminal vs non-terminal transition the retryable error arm takes). It keeps crash-safety because the job row is always left in a durable, next-tick-evaluable state (`Scheduled` with a future `scheduled_for`, or terminal `Failed`).

**Verify:** extend `upload_service.rs` tests — the existing `RecordingUploadAdapter::failing(UploadAdapterError::NetworkOrServer{..})` pattern (test infra at line 334) drives this. Add: (a) first network failure → job back to `Scheduled` with bumped `scheduled_for` and a `RetryQueued` event, NOT `Failed`; (b) after `max_attempts` exhausted → `Failed` with event; (c) `MediaConstraintFailed` still terminal on first failure. Run `cargo test -p montage-social upload_service::`.

### Step 3 — Add a token-freshness check + refresh-driver seam to the domain crate

**Files:** `crates/social/src/token.rs`, `crates/social/src/store.rs` (trait), and the worker path in `crates/social/src/upload_service.rs`.

`TokenSecret` already carries `access_token_expires_at` and `refresh_token_expires_at` (`token.rs` lines 11-12). Add a pure helper `TokenSecret::is_access_expiring(now, skew_secs) -> bool` (true when `access_token_expires_at <= now + skew`). Add a `refresh_token_exhausted(now)` helper (true when `refresh_token_expires_at <= now`).

Define a `TokenRefresher` trait (new, small) in the crate — e.g. `fn refresh(&self, account_id, secret: &TokenSecret, now) -> Result<TokenSecret, TokenRefreshError>` — that the *server* implements in Phase 2's OAuth module (calls the provider token endpoint with `client_secret`). The domain crate only depends on the trait, never on `reqwest`/`client_secret`, preserving the crate's "no live HTTP" property and the invariant that secrets live only server-side.

In `UploadService::execute_claimed_job`, right before the existing `let _token_secret = store.token_secret_for_account(...)` (line 89): if `is_access_expiring`, call the injected `TokenRefresher`; on success persist via `store.save_token_secret`; on failure where `refresh_token_exhausted` or provider says invalid_grant, flip the account to `ConnectedAccountStatus::NeedsReauth` (the enum variant already exists — see `sqlite_store.rs` line 706 and `upload_service.rs` line 277) via `store.save_connected_account`, append a `RequiresAction` event, and return `RequiresAction` for the job (do NOT hammer the provider — matches the spec's "stop hammering, surface for reconnect").

Why: the spec requires a fresh token at fire-time even while the user is offline, and `NeedsReauth` on refresh failure. The account re-check at the top of `execute_claimed_job` (lines 67-83) already short-circuits a `NeedsReauth` account on the *next* tick, so flipping the status is sufficient to stop retries.

**Verify:** add tests using a fake `TokenRefresher` (success / invalid-grant) and the existing in-memory store: (a) expiring token → refresher called, new secret saved, upload proceeds; (b) refresh invalid-grant → account becomes `NeedsReauth`, job `RequiresAction`, adapter never called; (c) non-expiring token → refresher NOT called. `cargo test -p montage-social`.

### Step 4 — Add the HTTP worker-tick endpoints on the Rust service

**File:** the service binary/crate created in Phase 1 (per the spec's "montage-social deployed as a small HTTP service"; today there is no `main.rs` for it — confirmed via workspace scan, only `crates/cli`, desktop, and codex bins exist). Assume Phase 1 added something like `crates/social-service/src/` (axum is already a workspace dep — `Cargo.toml` has `axum = "0.8"`, `sqlx` with `postgres`-capable features, `tokio`). Phase 4 adds two routes to that service:

1. `POST /internal/cron/run-due-jobs` — the minute-tick worker. Body/params: `{ now, batch_limit }`. Implementation: open the Postgres-backed `SocialStore` (Phase 1), call `PublishService::claim_due_jobs(store, now, batch_limit)`; for each claimed job, build an `ExecuteUploadRequest` (resolve title/description/tags/thumbnail from the campaign/account — reuse `resolve_account_default_privacy` and account defaults already in `api.rs`), ensure a fresh token via the Phase 2 `TokenRefresher` (Step 3), then call `SocialApi::execute_claimed_upload_job` with the real Phase 3 YouTube adapter. Return a JSON summary `{ claimed, published, processing, requeued, requires_action, failed }`.

2. `POST /internal/cron/poll-processing` — drives `SocialApi::poll_upload_status` for jobs currently in `Processing` (YouTube resumable upload returns `processing=true` until the video is done — see `upload_service.rs` lines 139-152). Needs a store query for `status=Processing` jobs; if Phase 1's Postgres store does not already expose one, add a `processing_publish_jobs(limit)` method to the `SocialStore` trait (`crates/social/src/store.rs`) with in-memory + Postgres impls, mirroring `claim_due_publish_jobs`.

3. `POST /internal/cron/refresh-tokens` — the token-refresh sweep. Query accounts whose token is near expiry (add a `SocialStore` method `accounts_with_expiring_tokens(now, skew, limit)` returning `(account_id, TokenSecret)` pairs), run the `TokenRefresher` for each, persist refreshed secrets, flip refresh-failures to `NeedsReauth`. This is independent of due-job firing so a due post never finds a dead token.

Secure these `/internal/*` routes with a shared secret header (e.g. `X-Cron-Token`) injected by Supabase (a Vault secret) so only pg_cron/pg_net or the Edge Function can call them — they are not public user routes (consistent with `execute_claimed_upload_job` being documented as "not a public user route", `api.rs` line 641).

**Reused vs new:** `claim_due_jobs`, `execute_claimed_upload_job`, `poll_upload_status`, all FSM/safety logic = reused unchanged. New = the route handlers, the shared-secret guard, the two new store query methods, and the per-job orchestration loop (claim → refresh → execute → record summary).

**Verify:** integration tests in the service crate against a local Postgres (the spec mandates this; `sqlx` test harness or `wiremock` for the provider). Seed a due `Scheduled` job + connected account + token, `POST /internal/cron/run-due-jobs`, assert the job ends `Published` (or `Processing`) and events are appended. Add a failure-injection test: provider 5xx → job requeued with future `scheduled_for`; repeat to exhaustion → `Failed`. Add an auth test: missing/wrong `X-Cron-Token` → 401. Run the service crate's `cargo test` and `cargo clippy` (workspace lints are strict — `unwrap_used = "deny"`, etc.).

### Step 5 — Create the pg_cron jobs and pg_net glue in Supabase

**Files:** new SQL migration(s) in the Supabase migrations directory created in Phase 1 (e.g. `supabase/migrations/<ts>_phase4_cron.sql`). This is the *only* truly new infra piece and the literal "previously-missing scheduler".

Add three `cron.schedule(...)` entries:

1. **Minute-tick firing** — `cron.schedule('social-run-due-jobs', '* * * * *', $$ ... $$)`. The job body uses `pg_net` (`net.http_post`) to POST to the service's `/internal/cron/run-due-jobs` with the `X-Cron-Token` header (read from Supabase Vault), passing `now = extract(epoch from now())` and a `batch_limit`. Alternatively, if Phase 1 decided on an Edge Function intermediary (open question in the spec), the cron calls the Edge Function URL which forwards to the service — keep the body identical, just swap the target URL. Choose one and document it.
2. **Poll-processing** — same shape on a slightly longer interval (e.g. every minute or every few minutes) hitting `/internal/cron/poll-processing`, so `Processing` jobs advance to `Published` once the provider finishes.
3. **Token-refresh sweep** — `cron.schedule('social-refresh-tokens', '*/5 * * * *', ...)` hitting `/internal/cron/refresh-tokens` with a skew window large enough that no due post finds a dead token (e.g. skew = 10-15 min; note TikTok access token = 24h per the spec, so a frequent sweep matters once TikTok lands in Phase 6).

Pin the service base URL and cron token as Supabase Vault secrets / DB settings, not literals in the migration. Include the matching `cron.unschedule(...)` in a paired down-migration for reversibility.

**Crash-safety note to encode here:** because every job is a durable Postgres row and `claim_due_publish_jobs` only re-selects `status=Scheduled`, a worker crash mid-tick leaves jobs either still `Scheduled` (re-picked next minute) or `Uploading`/`Processing` (left for the cancel-race/poll path) — the next tick re-evaluates. Add a brief migration comment documenting this so future maintainers don't add a fragile "stuck Uploading" cleanup without understanding the invariant. (Optional hardening, flag as open risk: a reaper that requeues `Uploading` jobs idle beyond a timeout — not strictly required for Phase 4 correctness.)

**Verify:** in a Supabase branch/local stack, insert a due `Scheduled` job, wait one minute (or call the cron function manually via `SELECT cron.schedule`-triggered run / direct `net.http_post`), and confirm the job transitions and events land. Verify the cron token is required. Confirm `cron.job` / `cron.job_run_details` show successful runs.

### Step 6 — End-to-end private-mode demo wiring and docs

**Files:** a short runbook under `docs/superpowers/plans/` (sibling to the existing plan files like `2026-06-03-social-upload-adapters.md`) describing: deploy service → set Vault secrets → apply Step 5 migration → seed a connected YouTube account (Phase 2) → schedule a job (existing `social_schedule_target` desktop command, `apps/desktop/src-tauri/src/commands/social.rs` line 279) → observe it fire via cron in YouTube *private* mode (audit gate, per the spec table).

Confirm the desktop poll path still works unchanged: `social_poll_status` / `social_publish_job` commands (`commands/social.rs` lines 427, 303) already read job status; with the server now firing autonomously, the desktop simply observes `Published`/`Failed` even if it was closed when the post fired. No desktop code change is required in Phase 4 (the desktop rewire to talk to the *server* store instead of local SQLite is Phase 5).

**Verify:** manual e2e in private mode; capture `cron.job_run_details` + a published private video URL as evidence.

---

### Reused vs newly built (summary)

- **Reused unchanged:** `claim_due_publish_jobs` (select-due + claim + attempt bump), `execute_claimed_upload_job`, `poll_upload_status`, the full `UploadService` safety stack (account re-check, cancel-race, privacy resolution), event append helpers, the Phase 3 real YouTube adapter, the `SocialStore` trait shape.
- **Newly built:** `retry_with_backoff_or_fail` (job.rs), retryable-vs-terminal branching in `upload_service.rs`, token-freshness helpers + `TokenRefresher` seam + `NeedsReauth`-on-refresh-failure, two/three `SocialStore` query methods (`processing_publish_jobs`, `accounts_with_expiring_tokens`) with in-memory + Postgres impls, the three `/internal/cron/*` service routes + shared-secret guard, the three `pg_cron` jobs + `pg_net` glue migration.

## Open risks
- Phase 1's invocation decision (pg_cron+pg_net directly vs Edge Function intermediary) is still open in the spec; Step 5 supports either but the exact target URL/auth shape must be finalized once Phase 1 lands.
- Phase 2's token-refresh implementation shape is assumed (a server-side TokenRefresher using client_secret). If Phase 2 instead stores refreshed tokens elsewhere or uses a different signature, Step 3's trait seam must be adjusted to match.
- Stuck-job reaper: a worker crash between claim (status=Uploading) and a terminal write leaves an Uploading row that claim_due_publish_jobs will NOT re-pick (it only selects Scheduled). Phase 4 relies on the poll-processing path and cancel-race re-read; whether to add an explicit timeout-based requeue of stale Uploading jobs is deferred — flag for a follow-up if observed in practice.
- Idempotency under at-least-once cron + crash: if the service publishes to YouTube but crashes before persisting Published, a re-pick could double-post. The existing idempotency_key (job.rs line 199) addresses duplicate scheduling but NOT duplicate provider calls; confirm whether YouTube's resumable session URI (Phase 3) is reused across attempts to dedupe, or add a provider-side dedupe check before re-upload. Decide during implementation.
- Backoff parameters (max_attempts, base, ceiling) are policy choices not fixed by the spec ('bounded retry with backoff'); pick defaults (e.g. 5 attempts, 60s base, capped at ~1h) and make them configurable via ExecuteUploadRequest.
- YouTube 100-uploads/day-per-project cap (spec open question): the minute-tick batch_limit and overall daily volume must stay under it; fine for solo use, flagged for multi-user.
- Concurrency: two overlapping minute-ticks could both claim if claim is not atomic. The SQLite/in-memory claim is single-process; the Postgres claim (Phase 1) must use SELECT ... FOR UPDATE SKIP LOCKED or a single UPDATE...RETURNING to be safe under concurrent cron invocations — verify Phase 1's implementation provides this or harden it here.
