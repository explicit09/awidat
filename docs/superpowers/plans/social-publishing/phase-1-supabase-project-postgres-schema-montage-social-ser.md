# Phase 1: Supabase project + Postgres schema + montage-social service deployment shape

> **BINDING:** Read `RECONCILIATION.md` first. Where this plan conflicts with it, RECONCILIATION wins. Key: domain stays sync (D1), server crate = `crates/social-server` (D2).


**Depends on phases:** []

## Prerequisites
- [USER] Create the Supabase project (org, name, region, strong DB password); capture project ref, region, DB password, service_role key, anon key, project URL.
- [USER] Create a Supabase Storage bucket for rendered artifacts (per D4) and capture its name + the storage service key, so the `/artifacts/upload-url` handshake can mint signed URLs.
- [USER] Enable pg_cron and pg_net at the project level (dashboard / ALTER DATABASE) — Supabase scopes pg_cron to the postgres database.
- [USER] Apply the committed SQL migrations against the Supabase project (supabase link + db push, or psql).
- [USER] Create the Fly.io org + app (fly apps create montage-social) and a deploy token; or Railway equivalent.
- [USER] Set deployment secrets via fly secrets set: DATABASE_URL (Supavisor session-pooler URL) and SERVICE_SHARED_SECRET. (GOOGLE_CLIENT_ID/SECRET + token-encryption key added in Phase 2.)
- [USER] Run the Step-8 pg_net smoke test in the Supabase SQL editor and confirm a 200 in net._http_response.
- [USER] Start the three platform app-review tracks in parallel (YouTube API TOS audit, TikTok video.publish, Instagram content publishing) — on the critical path, independent of this phase's code.
- [WE-STAGE] Author and commit the migrations, PgSocialStore + tests, the montage-social-server crate, Dockerfile, fly.toml, runbook, and CI changes.
- [WE-STAGE] Decide and document Fly.io as primary host with Railway as drop-in alternative; document Supavisor pooler as the secure DB path.

## Plan

## Phase 1 — Supabase project, Postgres schema, and the `montage-social` service deployment shape

### Goal and what "done" means
Stand up the durable, server-side foundation that every later phase builds on:
1. A Supabase project (Postgres + `pg_cron` + `pg_net` enabled) reachable by a Rust service.
2. A Postgres schema for the publishing domain that mirrors the existing, tested `SqliteSocialStore` schema row-for-row (same tables, same `payload_json` shape, same unique/secondary indexes), so the verified Rust domain logic ports without semantic drift.
3. A `PgSocialStore` implementation of the existing `SocialStore` trait (the only new Rust code in this phase) verified against a real Postgres with the exact assertions the SQLite store already passes.
4. A deployable `montage-social-server` binary (an Axum wrapper over the framework-neutral `SocialApi`) plus its deployment shape (where it runs, how Supabase invokes it, how it reaches Postgres securely), resolving the three Phase-1 open questions.

This phase does NOT implement real OAuth exchange (Phase 2), real YouTube upload (Phase 3), or the cron firing logic (Phase 4). It stands up infra and the store/transport seam those phases need. We deliberately deploy the service now (even though its adapters are still mocks) so deployment plumbing is proven independently of provider work.

---

### Resolution of the Phase-1 open questions (decisions this plan commits to)

**Q: How `montage-social` is invoked from Supabase — HTTP (pg_net/Edge Function) vs. a queue it polls?**
Decision: **HTTP, pull-shaped.** `pg_cron` runs a SQL job every minute that calls a single authenticated endpoint on the Rust service via `pg_net.http_post` (fire-and-forget). The Rust service, on that tick, does the claim-and-process itself by calling its own store (`claim_due_publish_jobs`) — i.e. the cron tick is just a "wake up and drain due work" trigger, the *authority* for claiming stays in Rust (`PublishJob::claim_for_upload`, already tested). Rationale: (a) reuses the already-tested `claim_due_publish_jobs` + FSM verbatim rather than reimplementing claim logic in SQL/TypeScript, which the design explicitly forbids; (b) `pg_net` is built into Supabase, no Edge Function runtime needed for the minute tick; (c) avoids standing up a separate queue product (SQS/PGMQ) for a solo-use cap of ~100 uploads/day. An Edge Function is reserved only for the OAuth browser-redirect callback in Phase 2 (it needs a public stable HTTPS URL on the Supabase domain); the scheduler path needs no Edge Function. We note PGMQ/Supabase Queues as the documented upgrade path if multi-user throughput ever demands it (Phase 7 concern).

**Q: Where the Rust service runs and how it reaches Supabase Postgres securely.**
Decision: **Fly.io**, single small shared-cpu-1x machine in a region close to the Supabase project region, reached by Supabase over public HTTPS (`pg_net` egress to the Fly app's `*.fly.dev` / custom domain). The service connects *back* to Supabase Postgres using the **Supavisor session pooler** connection string (port 6543, transaction or session mode) over TLS, with credentials held only in Fly secrets — never in the repo, never shipped to desktop. Rationale: Fly gives an always-on container with a stable public hostname and first-class secret management, matches the "always-on server" hard constraint, and `sqlx` (already in the workspace dep table) speaks Postgres over TLS to Supavisor without extra infra. Railway is documented as a drop-in alternative (same Dockerfile, same env contract) so the choice is not load-bearing. We do not use Supabase's direct DB port from outside; we go through Supavisor because it is the supported external-connection path and survives IPv4/IPv6 differences.

**Q (carried, flagged not solved): token-encryption mechanism** — explicitly deferred to Phase 2 per the design; this phase keeps the existing `TokenSecret` columns/shape so Phase 2 can swap the envelope without a schema migration.

---

### Prerequisites split

**[USER] human-only prerequisites (cannot be staged in code):**
1. Create the Supabase project (org, project name, region, strong DB password). Capture: project ref, region, DB password, `service_role` key, anon key, project URL.
2. Create the Fly.io org + app (`fly apps create montage-social`) and a Fly auth token for CI/deploy. (Or Railway equivalent.)
3. Start the three platform app-review tracks in parallel (YouTube API TOS audit, TikTok `video.publish`, Instagram content publishing) — independent of this phase's code but on the critical path per the design; this phase only records the requirement.

**[WE-STAGE] things we prepare in the repo now (no external account needed to author them):**
- The SQL migration files (schema + extension enablement + cron registration), authored and committed but applied by the user against their project.
- The `PgSocialStore` Rust implementation + integration tests.
- The `montage-social-server` binary crate (Axum) + Dockerfile + `fly.toml` + env-var contract, authored and committed; secrets injected by the user at deploy time.
- A `docs/` runbook describing the exact `supabase`/`fly` CLI commands the user runs.

---

### Step-by-step implementation

#### Step 1 — Author the Postgres schema migration mirroring `SqliteSocialStore`
- New directory `crates/social/migrations/` with `0001_publishing_schema.sql`.
- Mirror exactly the eight tables created in `crates/social/src/sqlite_store.rs` `create_schema()` (lines 32–120): `oauth_connections`, `connected_accounts`, `oauth_token_secrets`, `campaign_variant_targets`, `publish_jobs`, `publish_job_events`, `account_publish_defaults`, `workspace_member_roles`. Keep the same column names and the `payload_json` blob design so the Rust serde round-trip is byte-identical to today.
- Type mapping decisions (load-bearing, write them as comments in the SQL): `TEXT`→`text`; SQLite `INTEGER` epoch fields (`updated_at`, `scheduled_for`, `created_at`) → `bigint` (these are `i64` epochs in `model.rs`, NOT timestamps — do not convert to `timestamptz`, that would break the `i64` round-trip and the `claim_due_publish_jobs` comparison); `payload_json TEXT` → `jsonb` is tempting but keep `text` to preserve exact serde string equality the existing tests assert (e.g. `sqlite_account_listing_does_not_include_encrypted_token_material`); revisit jsonb later as a non-breaking change.
- Recreate the exact indexes: unique `connected_accounts_owner_provider_account` on `(owner_json, provider, provider_account_id)`; `campaign_variant_targets_variant`; unique `publish_jobs_idempotency_key`; `publish_jobs_due` on `(status, scheduled_for, id)`; `publish_job_events_job`. These indexes are what make `claim_due_publish_jobs` and the duplicate-detection tests pass — they must match.
- Verification: `psql` against a local Postgres (or `supabase db reset` locally) applies the migration cleanly; `\d+` on each table shows the expected columns/indexes.

#### Step 2 — Author extension + scheduler registration migration (no firing logic yet)
- `crates/social/migrations/0002_extensions_and_cron.sql`: `create extension if not exists pg_cron;` and `create extension if not exists pg_net;` (Supabase enables these in the `extensions`/`pg_catalog` schema; document that `pg_cron` must be enabled at the project level via dashboard/`ALTER DATABASE` since Supabase scopes it to the `postgres` database).
- Register a **disabled/no-op-safe placeholder** cron entry that documents the contract but does nothing destructive: a commented-out `cron.schedule('montage-publish-tick', '* * * * *', $$ select net.http_post(...) $$)` template with the service URL and auth header as placeholders. The actual enabling happens in Phase 4; Phase 1 only proves the extensions install and the template is correct. Rationale: keeps Phase 1 strictly infra; avoids a live cron hitting an endpoint that has no real adapter yet.
- Verification: extensions appear in `select * from pg_extension;`; the commented template is reviewed for correct `net.http_post` signature.

#### Step 3 — Add Postgres support to the `montage-social` crate
- Edit `crates/social/Cargo.toml` (**per D1 — NOT async sqlx**): add the **synchronous** `postgres` crate + `r2d2` + `r2d2_postgres` (new workspace deps; absent from Cargo.lock today — add them) for the blocking connection pool the sync store uses. Do NOT add async `sqlx` to `montage-social`. Keep `rusqlite` — the SQLite store stays for the desktop's local/editing use and as the reference for tests.
- Decision on async (**per D1 — SUPERSEDES any async-fork wording**): keep the existing **synchronous** `SocialStore` trait and `SocialApi` exactly as-is. `PgSocialStore` implements the **existing synchronous `SocialStore` trait** via the sync `postgres` crate + `r2d2` pool. The async server shell (`crates/social-server`) calls domain logic through `tokio::task::spawn_blocking(move || SocialApi::method(&mut store, ...))`, moving an owned pooled `PgSocialStore` (`'static + Send`) into the closure. No new `AsyncSocialStore`; no method implemented twice; `await_holding_lock` lint not triggered (blocking work runs off the async executor).
- Verification: `cargo build -p montage-social` compiles with the new deps.

#### Step 4 — Implement `PgSocialStore`
- New file `crates/social/src/pg_store.rs`, added to `crates/social/src/lib.rs` module list (after `sqlite_store`).
- Implement every method of the **existing synchronous `SocialStore` trait** (per D1) against an `r2d2`-pooled sync `postgres` client, with SQL that is the Postgres translation of each method in `sqlite_store.rs`: same `INSERT ... ON CONFLICT (...) DO UPDATE` upserts (Postgres native), same `WHERE`/`ORDER BY`, and crucially the same `claim_due_publish_jobs` semantics — select `status='scheduled' AND scheduled_for <= $now ORDER BY scheduled_for, id LIMIT $limit`, then for each call the existing `PublishJob::claim_for_upload(now)` (reused from `job.rs`, unchanged) and upsert. To make claiming concurrency-safe on the server (multiple ticks/instances), wrap the select in `FOR UPDATE SKIP LOCKED` inside a transaction — this is the Postgres upgrade over SQLite's single-writer model and is the correct primitive for the minute-tick worker. Map errors to the existing `SocialStoreError` variants (`DuplicateConnectedAccount` from the unique-index violation `23505`, `NotFound` from zero rows) so `SocialApi`'s error mapping in `api.rs` is unchanged.
- Reuse verbatim: all of `model.rs`, `job.rs` (FSM), `token.rs` (`TokenSecret` shape), the serde `payload_json` helpers. New code is only the SQL glue.
- Verification: see Step 5.

#### Step 5 — Integration tests for `PgSocialStore` against real Postgres
- New file `crates/social/tests/pg_store.rs` (integration test, gated behind an env var like `MONTAGE_TEST_PG_URL` so CI without Postgres still passes; locally point it at `supabase db` or a `docker run postgres`).
- Port the exact assertions from the `sqlite_store.rs` `#[cfg(test)]` block (lines 886–1176): round-trip oauth/account/token; persist targets/jobs/events with event-order and duplicate-idempotency-key rejection; `claim_due_publish_jobs` claims once in schedule order and respects the limit; duplicate provider account rejection; account listing contains no token material; disable checks owner; oauth status update persists. These are the behavioral contract; passing them against Postgres proves parity.
- Add two new Postgres-specific tests that SQLite couldn't exercise: (a) concurrent `claim_due_publish_jobs` from two pool connections claims each due job exactly once (proves `FOR UPDATE SKIP LOCKED`); (b) the unique-index `23505` maps to `DuplicateConnectedAccount`.
- Verification command: `MONTAGE_TEST_PG_URL=postgres://... cargo test -p montage-social --test pg_store`. Also run `cargo test -p montage-social` (no env var) to confirm the existing SQLite/in-memory suite is still green and untouched.

#### Step 6 — Create the `montage-social-server` binary crate (Axum wrapper, mock adapters)
- New crate `crates/social-server/` (per D2 — this is THE single server crate every later phase means; add to workspace `members`). Depends on `montage-social`, `tokio`, `reqwest` (for provider HTTP later), `tracing`/`tracing-subscriber`, and `axum` **with crate-local features `["http1","tokio","json"]`** (per G1 — the workspace `axum` is `default-features=false` and won't serve otherwise). The store deps (`postgres`/`r2d2`) come via `montage-social`. No async `sqlx`.
- `crates/social-server/src/main.rs`: read config from env (`DATABASE_URL` = Supavisor pooler URL, `BIND_ADDR`, `SERVICE_SHARED_SECRET`, `SOCIAL_FIRING_ENABLED` default **false** per G10, plus Supabase Storage creds for D4), build the `r2d2` Postgres pool (per D1 — not a `sqlx::PgPool`), apply migrations on boot (run the SQL migration files via the sync client, or a lightweight migrator — not `sqlx::migrate!`), construct `PgSocialStore`, and mount Axum routes that wrap `SocialApi` via `spawn_blocking` (per D1). For this phase the upload/status adapters are the existing `MockUploadAdapter` — real adapters arrive in Phase 3. Endpoints to expose now: `GET /health`, `GET /providers`, `GET /accounts`, the OAuth start/complete stubs, bind/validate/schedule, job status, **`POST /artifacts/upload-url`** (per D4 — returns a Supabase Storage signed PUT URL + the object ref that becomes `artifact_ref`; plus a resolve path that mints a short-lived signed GET URL for provider adapters to fetch in Phases 3/6), and `POST /internal/tick` (the cron target — calls `claim_due_publish_jobs` + `execute_claimed_upload_job` against the mock adapter, **but no-ops unless `SOCIAL_FIRING_ENABLED=true`** per G10, so a stray cron can't drive real-looking jobs through the mock before Phases 2–4). Protect `/internal/tick` with the `SERVICE_SHARED_SECRET` bearer header that `pg_net` will send.
- Reuse: `SocialApi` (framework-neutral, designed for exactly this per `api.rs` docstring lines 1–10), `ProviderRegistry::default_multi_platform`, the request/response DTOs already defined in `api.rs`. No business logic in the server crate.
- Verification: `cargo build -p montage-social-server`; a local run with `DATABASE_URL` pointing at local Postgres responds 200 on `/health` and `/providers` (curl); `POST /internal/tick` with the secret returns a drained-count, without the secret returns 401.

#### Step 7 — Containerize and define the deployment shape
- `crates/social-server/Dockerfile`: multi-stage Rust build (cargo build --release -p montage-social-server) → slim runtime image; expose `BIND_ADDR` port; copy the `crates/social/migrations/` directory so the boot-time migrator finds the SQL files at runtime.
- `crates/social-server/fly.toml`: app name `montage-social`, one shared-cpu-1x machine, internal port matching `BIND_ADDR`, a `[[services]]` http handler on 443, and an `[checks]` HTTP healthcheck hitting `/health`. Region set to match the Supabase project region (user fills in).
- Document the **secret contract** (set via `fly secrets set`, never committed): `DATABASE_URL` (Supavisor session-pooler URL with DB password), `SERVICE_SHARED_SECRET` (random, also stored in Supabase for `pg_net` to send). Phase 2 will add `GOOGLE_CLIENT_ID/SECRET` and the token-encryption key to this same contract.
- Verification: `docker build` succeeds locally; `fly deploy` (run by user) brings the machine up and `fly status` shows the healthcheck green; `curl https://<app>.fly.dev/health` returns 200 from the public internet.

#### Step 8 — Prove the Supabase→service network path (the open-question resolution, end to end)
- After the user has applied migrations and deployed the service: manually (or via the commented cron template from Step 2) run once in the Supabase SQL editor: `select net.http_post(url := 'https://<app>.fly.dev/internal/tick', headers := jsonb_build_object('Authorization','Bearer '||'<secret>'), body := '{}');` and confirm via `select * from net._http_response` that a 200 came back. This validates `pg_net` egress → Fly → service → Supabase Postgres (Supavisor) as a closed loop, which is the deployment-shape acceptance criterion for this phase. The cron *schedule* that calls this every minute is wired and enabled in Phase 4 (with real adapters); Phase 1 only proves the single manual invocation works.
- Verification: a 200 row in `net._http_response`; the service log (`fly logs`) shows the authenticated tick handled.

#### Step 9 — Runbook + CI
- New `docs/social-server/README.md` (or under `docs/superpowers/`): the exact ordered CLI commands the user runs — `supabase link`, applying `crates/social/migrations/*`, enabling `pg_cron`/`pg_net`, `fly apps create`, `fly secrets set`, `fly deploy`, and the Step-8 smoke test. Include the env-var contract table and the Railway-alternative note.
- Edit `.github/workflows/ci.yml`: add `cargo build -p montage-social-server` to the build matrix and run `cargo test -p montage-social` (existing fast suite). Optionally add a Postgres service container so the `pg_store` integration test runs in CI with `MONTAGE_TEST_PG_URL` set. Do not put any secrets in CI for the live Supabase project.
- Verification: CI passes on a branch; the runbook is reviewed end-to-end by following it once on the real project (the user does this).

---

### What is reused vs newly built
- **Reused unchanged:** `crates/social/src/model.rs`, `job.rs` (the whole FSM including `claim_for_upload`/`schedule`/`retry`/`fail`), `token.rs` `TokenSecret` shape, `api.rs` `SocialApi` + DTOs + error mapping, `provider.rs` registry, the SQLite store (`sqlite_store.rs`) which stays for desktop/local + as the test oracle, and the desktop command layer (`apps/desktop/src-tauri/src/commands/social.rs`) which is untouched in this phase.
- **Newly built:** the SQL migrations (`crates/social/migrations/`), `crates/social/src/pg_store.rs` + its async store trait seam, the `pg_store` integration tests, the `crates/social-server` Axum binary + Dockerfile + fly.toml, the runbook, and CI additions.

### How later phases depend on this
- Phase 2 (OAuth + encrypted tokens) writes into `oauth_connections`/`connected_accounts`/`oauth_token_secrets` created here, adds the OAuth-callback Edge Function pointed at this service, and swaps the token envelope behind the unchanged `TokenSecret` columns.
- Phase 3 (real YouTube upload) replaces the mock adapter mounted in `crates/social-server/src/main.rs`.
- Phase 4 (cron firing) enables the cron schedule whose template + `/internal/tick` endpoint + `claim_due_publish_jobs` path are stood up here.
- Phase 5 (desktop rewire) points the desktop at this service's HTTP endpoints instead of the local store.


## Open risks
- The existing SocialStore trait is synchronous (crates/social/src/store.rs); the server needs async sqlx. Plan introduces a parallel async store trait + thin async SocialApi driver. If that seam proves heavier than expected, the fallback is running sqlx via a blocking bridge (spawn_blocking), but that risks await-holding-lock lints (the workspace denies await_holding_lock). Validate the seam early in Step 3.
- Mirroring payload_json as Postgres `text` (not `jsonb`) preserves exact serde-equality the existing tests assert, but forfeits Postgres JSON querying. Acceptable now; flagged as a future non-breaking migration if cron/queries ever need to read inside the JSON.
- i64 epoch columns must be `bigint`, not timestamptz — a wrong choice silently breaks claim_due_publish_jobs comparisons and the FSM round-trip. Called out explicitly in Step 1 but easy to get wrong.
- pg_cron in Supabase is scoped to the postgres database and requires project-level enablement; if the user's plan/tier restricts pg_cron or pg_net egress, the HTTP-pull design needs the Edge Function fallback for the tick. Confirm pg_net egress to the Fly domain is allowed on the user's Supabase tier during Step 8.
- Supavisor pooler mode (transaction vs session) interacts with sqlx prepared statements; transaction mode can break prepared-statement caching. May need session mode or sqlx statement-cache disabled. Decide during Step 6/8 connection testing.
- SERVICE_SHARED_SECRET is a static bearer between Supabase and the service; acceptable for a single-tenant internal tick but should be rotated and is not a substitute for the desktop<->server HTTPS auth that Phase 5 defines.
- Deploying the service in Phase 1 with mock adapters means the deployed /internal/tick does nothing real until Phase 3/4; ensure the cron schedule stays disabled (only the manual smoke test runs) to avoid a live every-minute no-op hammering the service before adapters exist.
- YouTube 100-uploads/day-per-project cap is unaddressed by this phase (per design) and constrains multi-user later; schema does not yet model per-user provider projects.
