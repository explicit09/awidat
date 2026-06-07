# Social API Facade Phase 6

## Summary

Phase 6 turns the verified `montage-social` domain crate into a server-ready API
facade. The repository still does not have a dedicated web-server crate, so this
phase does not add concrete HTTP routes. Instead, it creates route-shaped Rust
methods, request/response DTOs, and authorization gates that a future Axum,
Next, Tauri, or other server wrapper can mount directly.

The purpose is to make the pipeline usable as a server-backed product surface
without blurring the layers:

- Montage app auth decides which user or workspace is making the request.
- Social OAuth decides which creator account can receive the post.
- Provider adapters decide platform capability, upload, and status behavior.
- The API facade exposes only account display data, eligibility, publish state,
  audit state, and final URLs.

Provider token material remains server-internal and must never appear in API
responses, event metadata returned to callers, logs, or frontend state.

## Phase 6 Position In The Pipeline

Completed domain layers:

1. Server account foundation: account, OAuth session, token secret, target, job,
   provider, and event models.
2. Multi-provider OAuth and eligibility contracts for YouTube, TikTok, and
   Instagram.
3. Publish targeting and durable queue contracts.
4. Upload adapter contracts and YouTube-first mocked upload/status boundaries.
5. Team and agency controls: workspace roles, account defaults, and usage audit.

Phase 6 adds the API-shaped orchestration layer over those pieces. It should not
reimplement account, publish, upload, status, or team logic. It should compose
the existing services and enforce request-level ownership.

## Goals

- Expose the initial server API surface from the approved social OAuth design as
  framework-neutral Rust methods.
- Provide request/response DTOs that are safe to serialize over HTTP later.
- Add one actor/owner authorization boundary for user-owned and workspace-owned
  social accounts.
- Route account operations through `SocialAccountService`.
- Route campaign target and publish job operations through `PublishService`.
- Route worker upload execution through `UploadService`.
- Route worker status polling through `UploadStatusService`.
- Preserve Phase 5 team policy behavior for workspace account management and
  publishing actions.
- Prove through tests that API responses do not expose provider token material.

## Non-Goals

- Do not choose or add a concrete HTTP framework.
- Do not add route macros, server startup, middleware, cookies, sessions, or
  deployment code.
- Do not add desktop or web UI.
- Do not add live Google, TikTok, or Meta HTTP calls.
- Do not add production database migrations beyond `SqliteSocialStore`.
- Do not bypass existing domain services with duplicate API-only business
  logic.

## API Boundary

Create `crates/social/src/api.rs` and export it from `crates/social/src/lib.rs`.

The facade should define:

- `ApiActor`
- `ApiOwner`
- `SocialApi`
- `SocialApiError`
- account request/response DTOs
- publish request/response DTOs
- worker request/response DTOs

The API facade is not an instantiated server object. It can stay as static
service methods, matching the existing `SocialAccountService`, `PublishService`,
`UploadService`, and `UploadStatusService` style.

## Route-Shaped Methods

### Provider And Account Methods

These methods correspond to:

- `GET /social/providers`
- `GET /social/accounts`
- `POST /social/oauth/:provider/start`
- `GET /social/oauth/:provider/callback`
- `DELETE /social/accounts/:id`

Expected facade methods:

- `SocialApi::providers`
- `SocialApi::accounts`
- `SocialApi::oauth_start`
- `SocialApi::oauth_complete`
- `SocialApi::disconnect_account`

Responses may include:

- provider id and capability summary
- account id
- owner
- provider account id
- display name
- handle
- avatar URL
- account kind
- account status
- scopes
- capabilities
- eligibility
- verification timestamps

Responses must not include:

- encrypted access token
- encrypted refresh token
- plaintext token material
- KMS key ids
- token version
- OAuth raw state
- PKCE verifier references

### Campaign Publish Methods

These methods correspond to:

- `POST /campaigns/:campaign_id/variants/:variant_id/target`
- `POST /campaigns/:campaign_id/variants/:variant_id/validate`
- `POST /campaigns/:campaign_id/variants/:variant_id/schedule`
- `GET /publish-jobs/:id`
- `POST /publish-jobs/:id/cancel`
- `POST /publish-jobs/:id/retry`

Expected facade methods:

- `SocialApi::bind_target`
- `SocialApi::validate_target`
- `SocialApi::schedule_target`
- `SocialApi::publish_job`
- `SocialApi::cancel_job`
- `SocialApi::retry_job`

Responses may include:

- campaign id
- variant id
- connected account id
- provider
- platform fields
- validation state
- scheduled time
- publish job id
- publish job status
- attempt count
- provider post id
- provider post URL
- normalized error
- raw error reference
- requires-action reason
- public audit event summaries

Responses must not expose token secret rows or raw provider errors that may
contain sensitive account data.

### Worker Methods

These methods are not public user routes. They are the server worker boundary
for scheduled upload execution and provider processing polling.

Expected facade methods:

- `SocialApi::execute_claimed_upload_job`
- `SocialApi::poll_upload_status`

Worker methods do not accept `ApiActor`; they operate on already-claimed jobs
and delegate to the existing upload services. They still return sanitized job
responses only.

## Authorization Rules

`ApiActor` should represent the authenticated Montage user and the workspace
roles known by the caller. This is separate from social provider credentials.

User-owned resources:

- `OwnerRef::User(user_id)` is accessible only when `actor.user_id == user_id`.
- User-owned account management and publish actions are allowed only to the same
  user.

Workspace-owned resources:

- `TeamRole::Owner` and `TeamRole::Admin` can connect and disconnect accounts.
- `TeamRole::Owner`, `TeamRole::Admin`, and `TeamRole::Publisher` can schedule,
  cancel, and retry publish jobs.
- `TeamRole::Viewer` can read only if a later caller explicitly asks for read
  access; it cannot mutate account or publish state in Phase 6.

The facade should call or mirror `TeamPolicy` rather than inventing a separate
role matrix.

## Error Shape

`SocialApiError` should preserve domain error sources without leaking sensitive
provider data:

- `Store`
- `Account`
- `Publish`
- `Upload`
- `Status`
- `Team`
- `Unauthorized`

HTTP status mapping is deferred to the future concrete server wrapper. Suggested
mapping for that wrapper:

- `Unauthorized` -> `403`
- not found store errors -> `404`
- invalid state or owner mismatch -> `409` or `403`, depending on route
- provider or network errors -> `502` or `503`
- validation/action-required states -> `422`

## Testing Requirements

Phase 6 is complete only when these pass:

```bash
cargo test -p montage-social api::tests
cargo test -p montage-social
cargo clippy -p montage-social --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Focused tests must cover:

- provider and account listing without token material
- OAuth start and callback persistence
- disconnect owner checks
- campaign target bind, validate, and schedule
- publish job lookup, cancel, and retry authorization
- workspace publisher permissions
- worker upload execution
- worker status polling to published
- provider post id mismatch rejection through the API
- SQLite and in-memory parity for API flows

The existing stable-toolchain `imports_granularity` rustfmt warning is
non-blocking only when `cargo fmt --all -- --check` exits with code 0.

## Success Criteria

Phase 6 is done when:

- `crates/social/src/api.rs` exists and is exported.
- The initial server API surface has framework-neutral equivalents.
- API methods compose existing domain services instead of duplicating business
  logic.
- All user/workspace mutation paths pass through the Phase 5 ownership policy.
- Worker entrypoints preserve upload/status lifecycle protections from Phase 4B.
- API responses and event summaries are proven token-safe.
- In-memory and SQLite stores behave the same for API-level flows.

## Remaining Work After Phase 6

After Phase 6, the next layers are:

- concrete HTTP server crate or route wrapper
- production database migration strategy
- live Google/TikTok/Meta provider clients
- durable worker scheduling/runtime integration
- frontend account selection, scheduling, status, and audit UI
- provider app-review and sandbox/manual integration runs
