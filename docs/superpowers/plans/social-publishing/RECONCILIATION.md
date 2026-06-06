# Plan-set reconciliation — binding decisions

The 7 phase plans were verified and blocked (`VERIFICATION.md`) for one
load-bearing contradiction + 5 cross-phase issues + per-phase gaps. This file is
the **authoritative resolution**. Where any phase plan conflicts with a decision
here, **this file wins**. Apply these before executing any phase.

## D1 — Server execution model: sync domain core + async server shell (RESOLVED)

**Decision:** Keep `awidat-social` synchronous and untouched (the `SocialStore`
trait, `SocialApi`, `PublishService`, `UploadService`, the FSM — all the merged,
tested code stays as-is). The new server is async only at its shell.

- `PgSocialStore` implements the **existing synchronous `SocialStore` trait**
  (NOT a new async trait). Phase 1 must **delete** any "fork a parallel
  `AsyncSocialStore` + async `SocialApi` driver" idea — that was the wrong call
  and is the root contradiction.
- Postgres access from the sync store uses the **synchronous `postgres` crate**
  (tokio-postgres's blocking sibling) with a blocking connection pool (`r2d2` +
  `r2d2_postgres`), NOT async `sqlx`. This lets the sync trait stay sync with no
  bridge gymnastics and no `await_holding_lock` risk.
- The async server shell (`awidat-social-server`: axum + reqwest for provider
  HTTP) calls domain logic via `tokio::task::spawn_blocking(move || {
  SocialApi::...(&mut store, ...) })`. Provider HTTP that is async (reqwest)
  lives in the server/leaf clients; the FSM calls them via the adapter trait,
  and Phase 3's leaf `block_on` is consistent with this model.
- Net effect for later phases: **they keep adding methods to the synchronous
  `SocialStore`** (as Phases 2/3/4/6/7 already assume). No method is implemented
  twice. The earlier "async fork" wording in Phase 1 is void.

Consequence for Phase 1 Cargo: do **not** add async `sqlx`/`postgres` feature to
the workspace `sqlx`. Add `postgres` + `r2d2` + `r2d2_postgres` for the store,
and `axum` (with `http1` + `tokio` features — see G1) + `reqwest` + `tokio` for
the server crate.

## D2 — The server crate identity (RESOLVED)

There is exactly one new server crate: **`crates/social-server`** (binary,
async). Every phase that says "wherever Phase 1 put the server" means
`crates/social-server`. The live provider HTTP clients (Phase 3 YouTube, Phase 6
TikTok/Instagram) and the JWT/auth middleware (Phase 7) live here, keeping
`crates/social` HTTP-free and dependency-light.

## D3 — `workspace_member_roles` schema: single owner = Phase 1 (RESOLVED)

Phase 1 creates `workspace_member_roles` with the **payload_json shape mirroring
SQLite** (`workspace_id text, user_id text, payload_json text, PK(workspace_id,
user_id)`), preserving the `WorkspaceMemberRole` serde round-trip. **Phase 7 does
NOT redefine the table** with a flat `role` column. Phase 7 only adds: (a) any
RLS policy if desired, and (b) a by-user role *query* against the existing
payload_json shape (deserialize, filter). Delete Phase 7's table-redefinition
step.

## D4 — File transport (Supabase Storage signed URLs): single owner = Phase 1 (RESOLVED)

Phase 1 **owns standing up** the Supabase Storage bucket for rendered artifacts
**and** the server's signed-URL handshake endpoint
(`POST /artifacts/upload-url` → returns a signed PUT URL + the storage object ref
that becomes `artifact_ref`; `GET`/resolve → a short-lived signed GET URL the
provider adapters fetch). Phase 3's `ArtifactSource` and Phase 6's
`PULL_FROM_URL`/`video_url` consume this; Phase 5's desktop upload uses the
handshake. Add this to Phase 1 scope explicitly.

## D5 — YouTube quota counter: single owner = Phase 3, in Postgres (RESOLVED)

The 100-uploads/day/project cap counter is **created and owned by Phase 3** (not
assumed from Phase 1). Phase 3 adds a small `provider_upload_quota` table
(`project_key text, day date, count int, PK(project_key, day)`) via a migration
in `crates/social/migrations/`, incremented transactionally when an upload is
accepted, checked before `videos.insert`. Phase 1 does not create it; Phase 3's
plan must include the migration.

## D6 — Phase 2 → Phase 3 is a HARD ordering (RESOLVED)

Phase 3's server wiring requires Phase 2's AEAD token storage + refresh
entrypoint fully merged. **Delete Phase 3's "decrypt-only fallback / ship a
TODO" path.** Phase 3 server wiring does not begin until Phase 2 is merged. The
crate-level YouTube client code (no token dependency) may be written in parallel,
but the `AccessTokenResolver` (decrypt + refresh) depends on Phase 2.

## D7 — Phase 7 depends on Phase 4 too (RESOLVED)

Phase 7's auth middleware protects the `/internal/*` worker routes Phase 4
creates and reuses Phase 4's cron-secret scheme. Update Phase 7
`depends_on` to `[1, 2, 4, 5]`.

## Per-phase concrete fixes (RESOLVED)

- **G1 (Phase 1):** workspace `axum` is `default-features = false`
  (Cargo.toml:422). The `crates/social-server` axum dep must enable `http1` +
  `tokio` (+ `json`) features or it won't serve. Add as a crate-local feature
  set.
- **G2 (Phase 1):** schema mirrors the **denormalized + payload_json hybrid**
  exactly (e.g. `connected_accounts(id, owner_json, provider,
  provider_account_id, status, payload_json, updated_at)`), NOT a fully
  normalized schema. i64 epoch columns are `bigint` (never `timestamptz`).
  `payload_json` stays `text` (not `jsonb`) to preserve serde-string equality the
  existing tests assert.
- **G3 (Phase 2):** **Mandatory step** (not a risk note): add the YouTube read
  scope (`https://www.googleapis.com/auth/youtube.readonly` or
  `youtube.force-ssl`) to `scopes_for(YouTube)` in
  `crates/social/src/oauth_url.rs` so `channels?mine=true` channel-identity
  resolution works; otherwise `complete_oauth`'s consistency check fails.
- **G4 (Phase 3):** adding `force_private` + chunk-size config touches the
  existing `YouTubeUploadAdapter`/request types — budget this as a small edit to
  tested types (update the redaction/round-trip tests), not "only the leaf client
  is new."
- **G5 (Phase 4):** threading `max_attempts`/`base_backoff_secs` into
  `ExecuteUploadInput` (upload_service.rs:10-19) + `ExecuteUploadRequest`
  (api.rs) is a signature change to tested structs — every caller (incl. the
  desktop mock path) must update. Budget the blast radius, or carry backoff
  policy as server-crate config passed positionally rather than on the domain
  struct (preferred: keep the domain struct stable, hold backoff policy in the
  server worker).
- **G6 (Phase 5):** there is **no** per-field desktop config struct in
  `commands/config.rs`. Add `AWIDAT_SOCIAL_SERVER_URL` via `awidat_config::Config`
  (or an env var read at command time), not a non-existent config-field pattern.
- **G7 (Phase 5):** feature-gating the legacy `apps/desktop/src-tauri/src/publishing/`
  dir is a larger refactor — it's wired into `commands/publishing.rs` and
  `AwidatState.upload_queue` (state.rs:105) and the render-queue auto-upload TS
  tests. Confirm nothing live depends on `set_render_upload_targets` /
  `start_uploads_for_job` / `poll_upload_states` before gating; budget it.
- **G8 (Phase 4 concurrency):** the Postgres `claim_due_publish_jobs` (Phase 1
  `PgSocialStore`) MUST use `SELECT ... FOR UPDATE SKIP LOCKED` or a single
  `UPDATE ... RETURNING` so overlapping minute-ticks can't double-claim. Phase 1
  must implement the atomic claim; Phase 4 verifies it.
- **G9 (Phase 4 idempotency):** guard against double-posting if the service
  crashes after a provider call but before persisting `Published`. Reuse the
  YouTube resumable session URI across attempts (Phase 3) or add a provider-side
  dedupe check before re-upload. Decide in Phase 3/4 implementation; document the
  chosen mechanism.
- **G10 (Phase 1 op-guard):** the deployed `/internal/tick` against the mock
  adapter must be **code-guarded off** (env flag `SOCIAL_FIRING_ENABLED=false`
  default) until Phase 3/4, not just "keep cron disabled by discipline."

## Updated dependency DAG

```
1 → 2 → 3 → 4
1,2,3,4 → 5
1,2,3,4 → 6
1,2,4,5 → 7
```
(Phase 7 gains dependency on 4 per D7.)
