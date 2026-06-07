# Server-Backed Social Publishing — Architecture Design

**Date:** 2026-06-03
**Status:** Approved (brainstorming) — ready for implementation planning
**Supersedes the "desktop-local scheduler" idea** for the publishing-firing component.

## Problem

The merged social-publishing stack (`montage-social` crate + desktop UI) is an
architecturally-complete but **mock harness**: connecting an account writes stub
tokens, uploads go through `MockUploadAdapter`, and **nothing fires scheduled
jobs at all** — there is no scheduler/worker anywhere in the tree. It cannot let
a real person connect a real account and have a scheduled post go out.

This design defines the architecture that makes scheduled publishing real.

## Research basis (decisive)

A deep, adversarially-verified research pass (2026-06-03;
`reference_social_publishing_research` memory) established two hard constraints
that determine the architecture — not preferences, facts:

1. **The firing component must be server-side.** Refreshing OAuth tokens to post
   on a user's behalf requires the confidential `client_secret`, which cannot
   ship in a distributable desktop app. And posts must fire when the user's
   machine is closed, which only an always-on server can do. Every product does
   it this way (Buffer: per-minute cron → SQS → stateless workers; Postiz:
   Temporal). **Desktop-only publishing is not viable.**
2. **Platform app-review/audit gates are the long pole, not code.** YouTube
   forces API-uploaded videos to *private* until a TOS audit passes; TikTok
   forces *private + ~5 test users* until a `video.publish` approval + sandbox
   demo + content audit pass. These take calendar weeks outside our control and
   must be started in parallel, early.

Other verified facts shaping the design:
- Offline posting = OAuth **refresh tokens**, refreshed server-side, stored
  encrypted at rest; handle revocation/expiry (TikTok access token = 24h).
- YouTube = Data API v3 **resumable upload** (POST → Location session URI →
  resume via empty PUT + 308/Range, ≤256GB). Quota: 10,000 units/day,
  `videos.insert` = 1 unit but **capped at 100 calls/day** (~100 uploads/day per
  project). (The "1,600 units/upload → ~6/day" figure is a myth — refuted.)
- TikTok = server-side REST (`/v2/post/publish/video/init/`, `video.publish`).
- Instagram content-publishing assumed by analogy only — **verify separately.**

## Decisions (locked during brainstorming)

- **Target shape:** server-first now. The always-on server owns OAuth, token
  storage, the scheduler, and publishing. The desktop app becomes a UI client.
  (Earlier "desktop-now, hosted-later" was reversed once research showed the
  firing component cannot live on the desktop.)
- **Platform:** server-side firing via **Supabase** (Postgres + `pg_cron` +
  Edge Function glue).
- **Domain logic:** **Rust-as-a-service** — deploy the existing, tested
  `montage-social` crate as a small HTTP service the Supabase cron/functions
  call. Reuse the verified job FSM and merged review-fix safety logic; do not
  reimplement it in TypeScript.
- **First platform:** build **YouTube** end-to-end first (cleanest API, existing
  adapter skeleton, generous quota). Start **all three** platforms' app-review
  in parallel to absorb the calendar wait.
- **Failure handling:** bounded retry with exponential backoff (using the
  existing `attempt_count`), then terminal `Failed`; manual `retry_job` remains.
- **Scheduler firing:** `pg_cron` minute-tick claims due jobs server-side; the
  post fires whether or not the desktop app is open.

## Architecture

```
┌─ Desktop (Tauri) ──────────┐         ┌─ Supabase ─────────────────────────┐
│ • Capture / edit / render  │         │ Postgres (publish jobs, accounts,   │
│ • Approve campaign         │  HTTPS  │   targets, events, encrypted tokens)│
│ • "Connect account" → opens│ ──────► │ Auth (user identity; later phase)   │
│   browser to server OAuth  │         │ pg_cron (minute tick: claim due)    │
│ • Upload rendered file     │         │ pg_cron (token refresh sweep)       │
│ • Poll job status          │ ◄────── │ Edge Function (thin HTTP/cron glue) │
└────────────────────────────┘         │        │ calls                      │
                                        │        ▼                            │
            client_secret + refresh ───┼─► montage-social service (Rust)       │
            tokens live ONLY here       │     job FSM + review-fix safety      │
                                        │     OAuth exchange/refresh           │
                                        │     YouTube/TikTok/IG adapters       │
                                        └─────────────────────────────────────┘
```

### Components

1. **Supabase Postgres** — durable store for publish jobs, connected accounts,
   campaign variant targets, job events, and **encrypted** OAuth tokens.
   Replaces local SQLite for the *publishing* domain. The desktop keeps its own
   local SQLite for editing/project state (unchanged).
2. **`pg_cron` minute-tick (scheduler)** — selects
   `scheduled_for <= now() AND status='scheduled'`, hands jobs to the worker.
   This is the previously-missing scheduler, as managed infra rather than a
   desktop tokio loop.
3. **`pg_cron` token-refresh sweep** — proactively refreshes tokens nearing
   expiry so a due post never finds a dead token.
4. **`montage-social` Rust service** — the existing crate deployed as a small
   HTTP service. Authoritative for the job lifecycle FSM and all merged safety
   logic (AI disclosure, account re-check, cancel-race, privacy, role gates).
   Cron worker + Edge Functions call it.
5. **OAuth + token module (server-side)** — holds `client_secret`; performs
   auth-code exchange and background refresh; stores tokens encrypted at rest.
   Absorbs the OAuth-exchange and token-encryption/KMS blockers.
6. **Provider adapters (server-side)** — real YouTube resumable upload first;
   TikTok + Instagram behind their audit gates.
7. **Desktop client** — initiates OAuth (opens browser to the server), uploads
   the rendered file to server storage, polls status. Holds no secrets.

**Invariant:** `client_secret` and refresh tokens exist only on the server.

## Data flow

### A) Connect an account (OAuth) — once per account
1. Desktop "Connect YouTube" → asks server for auth URL → opens system browser.
2. User consents → provider redirects to the **server's** callback with `code`.
3. Server exchanges `code` + `client_secret` for access + **refresh** tokens
   (the step currently stubbed), encrypts both at rest in Postgres. Desktop
   receives only "connected: @handle, eligible/not."

### B) Schedule a post
1. Desktop: approve campaign → upload rendered file to server storage → call
   "schedule job for `scheduled_for`."
2. Server (`montage-social`): validate target, run eligibility/role checks, write
   `publish_job` (`status=scheduled`), return job id. Desktop shows "Scheduled."

### C) Fire the post (previously missing entirely)
1. `pg_cron` minute-tick → selects due jobs → claims them (existing
   `claim_due_jobs` logic, now server-driven).
2. Per job: ensure a fresh access token (refresh if expired — works while the
   user is offline), then call the real provider adapter (YouTube resumable
   upload first).
3. Lifecycle runs the tested FSM: `scheduled → uploading → processing →
   published/failed`, with merged safety guards (disclosure stamped, account
   re-checked still-connected, cancel-race protected, privacy resolved); events
   appended for audit.
4. Failure → bounded retry with backoff via `attempt_count`, then `Failed`.
5. Desktop, whenever open, polls status and shows published/failed + provider
   URL. **The app need not be open for the post to fire.**

## Error handling & security

- **Upload failures:** exponential backoff retry via `attempt_count`; after N →
  terminal `Failed`; manual `retry_job` available.
- **Token-refresh failures** (revoked / expired-beyond-refresh): mark account
  `NeedsReauth`, stop hammering the provider, surface in desktop for reconnect.
  Google RISC revocation signals can wire in later.
- **Cancel race / disconnected account / privacy:** handled by the merged review
  fixes — same logic, now running server-side.
- **Crash safety:** jobs are durable Postgres rows with explicit status; a
  worker crash mid-upload leaves a recoverable row the next tick re-evaluates.
- **Secrets:** `client_secret` + refresh tokens only on the server, encrypted at
  rest (real authenticated encryption — envelope/KMS or libsodium/AES-GCM;
  exact mechanism is an open question). Replaces the XOR-with-hardcoded-key stub.
- **Transport:** desktop ↔ server is authenticated HTTPS; desktop receives only
  account metadata + job status (facade already redacts tokens — tested).

## Audit gates (start in parallel, immediately)

| Platform | Gate | Effect until passed |
|----------|------|---------------------|
| YouTube | API project TOS audit | Uploads forced **private** |
| TikTok | `video.publish` approval + sandbox demo + content audit | Posts **private**, ~5 test users |
| Instagram | `instagram_content_publish` review (Business/Creator) | *Verify separately* |

The full pipeline can be **built and demoed in private/sandbox mode** before any
audit clears. Public posting flips on per-platform as each audit passes.

## Testing strategy

- **`montage-social` Rust:** existing unit/e2e tests stay green (FSM unchanged,
  only its callers move server-side).
- **New server pieces** (OAuth exchange, real adapters, cron worker):
  integration tests against provider sandboxes + a local Postgres; explicit
  failure-injection tests for resumable-upload interruption and token-refresh
  failure.
- **Desktop client:** existing `node:assert` tests for client flows.

## Scope & sequencing

This is a multi-phase program; each phase is its own spec → plan → build cycle:

1. **Supabase project + Postgres schema** for the publishing domain + the
   `montage-social` service deployment shape.
2. **Server-side OAuth exchange + encrypted token storage** (YouTube first).
3. **Real YouTube resumable-upload adapter**, wired server-side.
4. **`pg_cron` scheduler + token-refresh sweep** firing jobs through the service.
5. **Desktop client rewire** (OAuth-via-browser, upload-to-server, poll status).
6. **TikTok + Instagram adapters** behind their audits.
7. **Multi-user auth** (Supabase Auth → the existing workspace/role model) — last.

## Open questions (for implementation planning)

- **Token encryption mechanism:** Supabase Vault / KMS envelope encryption vs.
  app-level libsodium/AES-GCM with a key from Supabase secrets. Decide in
  Phase 2.
- **How `montage-social` is invoked from Supabase:** HTTP service called by Edge
  Functions/`pg_cron` (`pg_net`), vs. a queue the service polls. Decide in
  Phase 1.
- **Where the Rust service runs** (Fly.io / Railway / a container) and how it
  reaches Supabase Postgres securely.
- **Instagram publishing flow** — never independently verified; confirm the
  Graph API container-then-publish flow + review gates before Phase 6.
- **YouTube 100-uploads/day-per-project cap** — fine for solo use; multi-user
  would need per-user projects or higher quota. Not solved now; flagged.
- **File transport** — Supabase Storage signed-URL upload from desktop vs.
  direct-to-provider; affects large-file handling.
