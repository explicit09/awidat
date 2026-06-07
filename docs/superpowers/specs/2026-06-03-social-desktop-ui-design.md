# Server-Backed Social Publishing — Desktop UI Design

## Summary

This is the first post-Phase-6 sub-project of the server-backed social
publishing pipeline. It puts a desktop surface on top of the verified
`montage-social` `SocialApi` facade so a user can connect a creator account,
schedule a campaign variant to publish, watch the job progress, and review the
audit trail — all driven by the server-owned domain layer rather than the
legacy desktop-local publishing stack.

There is **no HTTP layer yet**. The desktop bridges to `SocialApi` in-process
through Tauri commands, exactly as the existing desktop publishing commands
bridge to `apps/desktop/src-tauri/src/publishing/`. Because `SocialApi` is
framework-neutral, the same command bodies can later move behind an axum
wrapper unchanged.

This sub-project covers all four UI surfaces named in the Phase 6 spec:
**account connect/list, schedule, job status/monitoring, and audit/history.**

## Context: Two Stacks, "Replace As We Go"

The repo has two social-publishing implementations:

1. **Legacy desktop-local** — `apps/desktop/src-tauri/src/publishing/` with its
   own OAuth listener, OS-keychain token storage, providers, and upload queue;
   surfaced by `PublishingSettings.tsx`, `DeliverySurface.tsx`, and
   `CampaignApprovalPanel.tsx`. Tokens live on the desktop.
2. **Server-backed** — `crates/social/` (`montage-social`). The server owns
   identity, posting state, and encrypted tokens; the desktop receives only
   account IDs, display/eligibility data, and job status.

The chosen migration strategy is **replace as we go**: the new server-backed
desktop commands and React surfaces are built alongside the legacy ones, and
each legacy responsibility is retired as its server-backed equivalent lands and
is proven. This sub-project does the *first* replacement slice (account connect
+ schedule + status + audit) and explicitly retires the legacy equivalents it
supersedes; it does not touch legacy code paths it does not yet replace.

### What this sub-project retires

When the server-backed surfaces below are working and tested, these legacy
pieces are removed (or reduced to thin shims that call the new commands):

- `commands::publishing::{begin_provider_oauth, complete_provider_oauth,
  get_provider_status, disconnect_provider, list_providers}` → replaced by the
  `social_*` account commands.
- The `PublishingSettings.tsx` connect/disconnect/status UI → replaced by the
  new Accounts surface.

Legacy upload-queue and AI-disclosure paths that are NOT yet replaced
(`start_uploads_for_job`, `compute_ai_disclosure`, render-target wiring) stay
in place and are scheduled for later sub-projects (worker runtime, live
providers). No legacy file is deleted until its server-backed replacement is
green and the desktop is switched over in the same change.

## Goals

- Connect/list/disconnect creator accounts through `SocialApi`, kicking off the
  OAuth flow and showing status + eligibility.
- Schedule a campaign variant to a connected account: bind → validate →
  schedule, surfacing validation reasons.
- Monitor publish jobs with live status and cancel/retry.
- Review per-account audit: jobs, events, status counts, final URLs.
- Never expose token material to the frontend (proven by the facade; reinforced
  by command-response DTOs that mirror only `SocialApi` response types).
- Keep each Tauri command thin: translate args → `SocialApi` call → camelCase
  serde response. No business logic in the command layer.

## Non-Goals

- No HTTP/axum server (next sub-project).
- No live Google/TikTok/Meta clients — OAuth token exchange and uploads run
  against the existing mock adapter boundary; the OAuth *start* opens the real
  provider authorize URL, but *completion* uses the mocked/server token path,
  not a live exchange. (Live providers are a later sub-project.)
- No durable background worker — status advances when the UI (or a foreground
  poll) calls the worker commands; no daemon (later sub-project).
- No production database — the desktop uses `SqliteSocialStore` against a file
  in the app data dir; migrations beyond what the crate provides are out of
  scope.
- No team/workspace management UI — single-user (`OwnerRef::User`) only this
  pass; the actor/owner plumbing is built so workspace support drops in later.

## Architecture

```text
React 19 surfaces (Accounts | Schedule | Jobs | Audit)
        │  @tauri-apps/api invoke(...)
        ▼
Tauri commands  apps/desktop/src-tauri/src/commands/social.rs
   social_providers / social_accounts / social_oauth_start /
   social_oauth_complete / social_disconnect_account /
   social_bind_target / social_validate_target / social_schedule_target /
   social_publish_job / social_cancel_job / social_retry_job /
   social_execute_upload / social_poll_status
        │  thin translation only
        ▼
montage_social::api::SocialApi   (framework-neutral facade — DONE)
        │
        ▼
SqliteSocialStore  (file in app data dir)  +  mock upload/status adapters
```

### Where state lives

A `social` field is added to `MontageState`
(`apps/desktop/src-tauri/src/state.rs`):

```rust
pub social: Mutex<SqliteSocialStore>,
```

`SqliteSocialStore` is opened once at app startup against
`<app_data_dir>/social.sqlite` (a new helper `SqliteSocialStore::open(path)`
is added alongside the existing `new_in_memory`; it runs the same schema
setup). Commands lock the mutex, call `SocialApi`, and drop the lock. The
single-process desktop has no concurrent-writer problem; the mutex serializes
access, matching how `MontageState` already guards `codex`, `turn`, and `jobs`.

### The actor

This pass is single-user. The command layer constructs
`ApiActor::new(LOCAL_USER_ID, vec![])` and `ApiOwner::user(LOCAL_USER_ID)`,
where `LOCAL_USER_ID` is the fixed sentinel `"local-user"`. (A real per-user id
is deferred to when an identity service exists; using a constant now keeps the
store rows stable and the swap to a real id a one-line change.) The id is
threaded through every command rather than hard-coded inside `SocialApi` calls.

## Components

### 1. `SqliteSocialStore::open(path)` (crate change)

`crates/social/src/sqlite_store.rs` gains a file-backed constructor mirroring
`new_in_memory`'s schema initialization. This is the only `montage-social`
change in this sub-project; it is store-level, not new business logic.

### 2. Tauri command module `commands/social.rs`

One thin async command per `SocialApi` method. Each:
- builds `ApiActor` / `ApiOwner` from the local user id,
- locks `state.social`,
- calls the matching `SocialApi` function,
- maps the result to a `#[serde(rename_all = "camelCase")]` response struct
  (or reuses the facade's response DTOs directly via `serde`),
- maps `SocialApiError` to a `String` (or a structured `{ kind, message }`)
  error, following the existing `commands/publishing.rs` error convention.

Worker commands (`social_execute_upload`, `social_poll_status`) use the **mock**
`UploadAdapter` / `UploadStatusAdapter` this pass so the status lifecycle is
demonstrable end-to-end without live providers.

Commands are registered in `lib.rs`'s `generate_handler!`.

### 3. React surfaces (`apps/desktop/src/app/social/`)

A new folder, model-then-presentation split matching the existing
`publishingSettingsModel.ts` / `PublishingSettings.tsx` convention:

- `socialModel.ts` — types mirroring the command responses; pure derivation
  helpers (status string, primary-action picker, eligibility summary). JSX-free,
  unit-tested in the node harness like `publishing-settings.test.ts`.
- `SocialAccounts.tsx` — connect (opens authorize URL), list with status +
  eligibility dot+label, disconnect.
- `SocialSchedule.tsx` — pick account + time + platform fields, run
  validate, show reasons, schedule.
- `SocialJobs.tsx` — job list with live status, cancel/retry, a "refresh"/poll
  action that calls the worker commands.
- `SocialAudit.tsx` — per-account jobs/events/status-counts/final URLs.

These mount inside the existing Settings/Delivery shell (exact placement
follows current navigation; the Accounts surface replaces the legacy
Publishing connect section).

### UI aesthetic

Follows the established house style (see project memory): DaVinci-calm — muted
palette, hairline borders, **dot + label** for status (never colored pills),
content-dominant. Status dot maps: connected/published = neutral-positive,
processing/uploading = in-progress, needs-action/failed = attention; always
paired with a text label, never color alone.

## Data Flow (happy path)

1. **Connect:** `SocialAccounts` → `social_oauth_start` → persists an OAuth
   connection and returns the real provider authorize URL → desktop opens it in
   the browser. The user authorizes; the provider redirects to the local
   callback listener (the existing `127.0.0.1:8419` listener from the legacy
   `publishing/oauth_listener.rs` is reused as the redirect sink) which captures
   `code` + `state`. Because live token exchange is out of scope this pass,
   `social_oauth_complete` is then invoked with a **deterministic stub token
   bundle + connected-account profile** derived from the provider + connection
   (no real network call), exercising the full server-side persistence and
   `state`-hash validation path in `SocialApi::oauth_complete`. When the live
   provider sub-project lands, only the stub-bundle construction is swapped for
   a real exchange; the command and UI are unchanged.
2. **List:** `social_accounts` → account summaries (status, eligibility, scopes,
   no tokens).
3. **Schedule:** `social_bind_target` → `social_validate_target` (shows reasons
   if not valid) → `social_schedule_target` → a Scheduled job.
4. **Run:** `social_poll_status`/`social_execute_upload` (worker commands, mock
   adapters) advance Uploading → Processing → Published.
5. **Monitor/Audit:** `social_publish_job` and an audit command surface job
   state + events + final URL.

## Error Handling

- `SocialApiError::Unauthorized` → a distinct error kind the UI shows as "not
  permitted" (defensive; single-user rarely hits it).
- `Store(NotFound)` → "not found" empty/410 state.
- Validation `RequiresAction`/`Invalid` → surfaced inline with the reason codes
  the facade returns (e.g. `account_not_eligible`, `scheduled_time_invalid`),
  mapped to human copy in `socialModel.ts`.
- Provider/network and missing-URL errors → retryable error state on the job.
- Every command returns a typed error; the React layer never swallows.

## Testing

- **Crate:** a focused test for `SqliteSocialStore::open(path)` (round-trips an
  account through a real file, then reopens and reads it back).
- **Command layer (Rust):** per-command tests over an `MontageState` backed by a
  temp-file `SqliteSocialStore`, asserting the command translates correctly and
  responses carry no token material (serialize + substring check, mirroring the
  crate's token-safety tests).
- **Model (TS):** `social.test.ts` in the existing node harness exercises
  `socialModel.ts` derivations (status strings, primary action, eligibility
  copy, reason-code → human mapping) with no React/Tauri.
- **End-to-end (Rust):** one command-level test driving connect → schedule →
  poll → published over the file store, asserting the terminal job + audit.
- React component rendering is validated by the existing desktop smoke harness
  where it already covers Settings/Delivery; no new e2e browser harness.

## Sequencing (for the implementation plan)

1. `SqliteSocialStore::open(path)` + crate test.
2. `commands/social.rs` account commands (providers/accounts/oauth/disconnect) +
   `MontageState.social` wiring + registration; retire legacy connect commands.
3. `SocialAccounts.tsx` + `socialModel.ts` + model tests; swap legacy connect
   UI.
4. Publish commands (bind/validate/schedule/publish_job/cancel/retry) +
   `SocialSchedule.tsx` + `SocialJobs.tsx`.
5. Worker commands (execute/poll, mock adapters) + status advance in `SocialJobs`.
6. Audit command + `SocialAudit.tsx`.
7. Verification: `cargo test`/`clippy`/`fmt` for the crate + desktop crate;
   desktop TS tests; manual smoke of the four surfaces.

## Remaining Work After This Sub-Project

- HTTP/axum route wrapper (the command bodies lift directly onto it).
- Live Google/TikTok/Meta OAuth exchange + upload + status clients.
- Durable background worker (replaces the foreground poll commands).
- Team/workspace ownership UI.
- Full deletion of the legacy `publishing/` module once every responsibility is
  replaced.
