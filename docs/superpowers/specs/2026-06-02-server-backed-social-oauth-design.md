# Server-Backed Social OAuth Design

## Summary

Awidat needs a server-owned social account layer so web users can connect their
own YouTube, TikTok, and Instagram accounts, then schedule campaign posts
through those accounts. This auth is separate from Codex/OpenAI auth. Codex auth
answers "who powers the agent"; social OAuth answers "which creator account can
receive this post."

This design chooses the ambitious path: build the multi-platform OAuth
foundation for YouTube, TikTok, and Instagram now. YouTube can still be the
first fully working upload adapter, but TikTok and Instagram must have real
OAuth session, account registry, permission, eligibility, and job-target
contracts from the beginning.

## Goals

- Let every Awidat user connect their own YouTube, TikTok, and Instagram
  accounts through backend OAuth flows.
- Store social tokens encrypted on the server, never in project files or
  desktop-local campaign manifests.
- Attach connected social accounts to campaign variants and publish jobs.
- Make provider capabilities and account eligibility visible before scheduling.
- Support both single-user creators and future team/workspace ownership.
- Keep provider-specific OAuth, refresh, validation, upload, scheduling, and
  status polling isolated behind adapters.
- Preserve audit history for account connection, token refresh, publish
  attempts, user action requirements, and final URLs.

## Non-Goals

- Do not merge social OAuth with the existing Codex/OpenAI auth flow.
- Do not make the desktop app the durable scheduler or token vault.
- Do not promise fully automated TikTok or Instagram publishing before platform
  permissions, account eligibility, and app review requirements are satisfied.
- Do not store raw provider tokens in campaign manifests, logs, analytics, or
  frontend state.
- Do not build a generic social scheduler detached from campaign variants and
  Awidat's render/proof pipeline.

## System Boundary

The server owns social identity and posting state:

```text
Awidat user/session
  -> OAuth connection session
    -> Connected social account
      -> Campaign variant target
        -> Publish job
          -> Provider adapter
```

The desktop app and web app can initiate OAuth and request publish/schedule
actions, but they receive account IDs, display metadata, eligibility state, and
job status only. They do not receive refresh tokens or long-lived access tokens.

## Data Model

### `connected_accounts`

One row per connected social identity.

- `id`
- `owner_type`: `user` or `workspace`
- `owner_id`
- `provider`: `youtube`, `tiktok`, or `instagram`
- `provider_account_id`
- `display_name`
- `handle`
- `avatar_url`
- `account_kind`: channel, creator, business, professional, page, or unknown
- `status`: `connected`, `needs_reauth`, `missing_scope`, `ineligible`,
  `disabled`, or `revoked`
- `scopes`
- `capabilities`
- `eligibility`
- `last_verified_at`
- `created_at`
- `updated_at`

Uniqueness should prevent the same provider account from being connected twice
to the same owner unless explicit multi-workspace linking is later supported.

### `oauth_connections`

Short-lived OAuth handshakes.

- `id`
- `owner_type`
- `owner_id`
- `provider`
- `state_hash`
- `pkce_verifier_ref` where the provider supports PKCE
- `redirect_uri`
- `requested_scopes`
- `return_to`
- `status`: `started`, `completed`, `failed`, or `expired`
- `error_code`
- `created_at`
- `expires_at`

The raw `state` value is never stored directly. Store a hash and compare on
callback.

### `oauth_token_secrets`

Encrypted provider token material.

- `id`
- `connected_account_id`
- `encrypted_access_token`
- `encrypted_refresh_token`
- `access_token_expires_at`
- `refresh_token_expires_at`
- `token_version`
- `kms_key_id`
- `last_refreshed_at`
- `created_at`
- `updated_at`

Token rows are server-internal. Application APIs should never return them.

### `campaign_variant_targets`

The server-side binding between an Awidat campaign variant and a social account.

- `id`
- `campaign_id`
- `variant_id`
- `connected_account_id`
- `provider`
- `platform_fields`
- `scheduled_for`
- `validation_state`
- `created_at`
- `updated_at`

Desktop-local campaign manifests may keep `variant_id`, `provider`, and a
server `connected_account_id`, but never token material.

### `publish_jobs`

Durable execution records.

- `id`
- `campaign_id`
- `variant_id`
- `connected_account_id`
- `provider`
- `artifact_ref`
- `idempotency_key`
- `scheduled_for`
- `status`
- `attempt_count`
- `provider_post_id`
- `provider_post_url`
- `normalized_error`
- `raw_error_ref`
- `requires_action_reason`
- `created_by`
- `created_at`
- `updated_at`

Publish job states:

```text
draft
validated
scheduled
uploading
processing
published
failed
requires_action
cancelled
```

### `publish_job_events`

Append-only audit history.

- `id`
- `publish_job_id`
- `event_type`
- `actor_type`: user, system, worker, or provider
- `message`
- `metadata`
- `created_at`

## Provider Adapter Contract

Each provider implements the same server-side interface:

- `begin_oauth(owner, return_to) -> authorization_url`
- `complete_oauth(callback) -> connected_account`
- `refresh_token(account) -> token_state`
- `fetch_account_profile(account) -> profile`
- `fetch_capabilities(account) -> capabilities`
- `validate_variant(target, artifact) -> validation_result`
- `publish_now(job) -> provider_post`
- `schedule_or_queue(job) -> provider_post_or_queue_state`
- `poll_status(job) -> provider_status`
- `revoke(account) -> revoke_result`
- `normalize_error(error) -> normalized_error`

The adapter returns normalized capability and eligibility data rather than
making the UI know provider rules.

## Provider Requirements

### YouTube

Expected first complete publish adapter.

- OAuth through Google.
- Connect a channel identity, not just a Google account identity.
- Store refresh token and granted scopes.
- Validate upload permission, channel status, video duration, privacy, title,
  description, tags, thumbnail, audience fields, and scheduled time.
- Support long-form and Shorts uploads.
- Support thumbnail upload where allowed.
- Support privacy and scheduled publishing where the API permits it.
- Poll processing status and record final video URL.

### TikTok

Build real account OAuth and eligibility now; upload capability depends on app
approval and permission availability.

- OAuth through TikTok's developer flow.
- Fetch creator/account info and posting eligibility.
- Track available privacy options and direct-post permissions.
- Validate duration, aspect ratio, caption, disclosure, and privacy settings.
- Require explicit user consent when TikTok requires it.
- Support `requires_action` for app review, missing direct-post permission,
  creator ineligibility, or required user confirmation.
- Implement upload and status polling behind the shared adapter when approved.

### Instagram

Build real account OAuth and eligibility now; upload capability depends on
professional account/page permissions and Meta app review.

- OAuth through Meta.
- Resolve the publishable Instagram professional account, not only the Facebook
  login identity.
- Validate professional/business account eligibility, connected page access,
  media type, duration, aspect ratio, caption, and Reels/feed destination.
- Model media container creation, publish, and status polling in the adapter.
- Support `requires_action` for missing page linkage, missing professional
  account, missing scopes, or app-review limitations.

## API Surface

Initial server APIs:

- `GET /social/providers`
- `GET /social/accounts`
- `POST /social/oauth/:provider/start`
- `GET /social/oauth/:provider/callback`
- `DELETE /social/accounts/:id`
- `POST /campaigns/:campaign_id/variants/:variant_id/target`
- `POST /campaigns/:campaign_id/variants/:variant_id/validate`
- `POST /campaigns/:campaign_id/variants/:variant_id/schedule`
- `GET /publish-jobs/:id`
- `POST /publish-jobs/:id/cancel`
- `POST /publish-jobs/:id/retry`

Server responses expose account display data, capability data, validation
messages, and job state. They never expose provider tokens.

## Account Selection UX

The campaign approval surface should show connected social accounts as
publishable destinations:

- Provider icon/name.
- Account handle and display name.
- Eligibility state.
- Missing permission or app-review warnings.
- Native scheduling support vs server queue scheduling.
- Per-platform validation issues before approval.

If a target platform has no connected account, the user sees a connect action.
After OAuth succeeds, the campaign variant can bind to that account without
rebuilding the campaign.

## Scheduling and Queue Behavior

Scheduled posting is server-side:

- The worker claims due jobs by `scheduled_for`.
- Each job uses an idempotency key derived from campaign, variant, account, and
  artifact version.
- Retry policy depends on normalized error category.
- Token refresh happens before publish if the access token is near expiry.
- Expired, revoked, or under-scoped accounts move jobs to `requires_action`.
- Provider-native scheduling is used when reliable and supported; otherwise the
  Awidat queue waits and publishes at the requested time.

## Security

- Encrypt token material with envelope encryption or KMS-backed keys.
- Hash OAuth state and expire connection sessions quickly.
- Use PKCE where supported.
- Never return provider tokens to frontend clients.
- Redact provider tokens from logs, errors, traces, and analytics.
- Keep raw provider errors behind server-only references when they may include
  sensitive account data.
- Gate account access by owner and workspace membership on every request.
- Record account connect, refresh failure, revoke, publish, retry, and cancel
  events.

## Error Handling

Normalize provider errors into product states:

- `token_expired`
- `refresh_failed`
- `missing_scope`
- `provider_app_not_approved`
- `account_not_eligible`
- `account_revoked`
- `media_constraint_failed`
- `scheduled_time_invalid`
- `rate_limited`
- `daily_post_cap_reached`
- `platform_processing_failed`
- `manual_review_required`
- `network_or_server_error`

Every failed or blocked job should include:

- User-facing reason.
- Recommended next action.
- Whether automatic retry is allowed.
- Original provider error reference for support.

## Testing Strategy

- Unit-test OAuth state creation, callback validation, and expiry.
- Unit-test token encryption/decryption through a test key provider.
- Unit-test account ownership checks.
- Unit-test provider capability normalization for YouTube, TikTok, and
  Instagram.
- Unit-test validation rules without live platform APIs.
- Add adapter contract tests with mocked provider responses.
- Add queue tests for due-job claiming, idempotency, retry, cancellation, and
  `requires_action`.
- Add API tests that prove provider tokens never appear in responses.
- Use live sandbox/test accounts only in explicit manual or gated integration
  runs.

## Rollout Plan

### Phase 1: Server Account Foundation

- Add server-side connected account, OAuth session, token secret, target, job,
  and event models.
- Add token encryption abstraction with a local/test key provider.
- Add provider registry and adapter trait.
- Add API endpoints for provider listing, account listing, OAuth start/callback,
  and disconnect.

### Phase 2: Multi-Provider OAuth and Eligibility

- Implement YouTube OAuth, channel profile, refresh, and capability fetch.
- Implement TikTok OAuth, profile, creator info, eligibility, and capability
  fetch.
- Implement Instagram OAuth, professional account resolution, eligibility, and
  capability fetch.
- Surface connected accounts and eligibility in campaign approval UI.

### Phase 3: Publish Job Targeting

- Bind campaign variants to connected accounts.
- Validate variants through provider adapters before scheduling.
- Create durable publish jobs with idempotency keys and audit events.
- Add queue worker behavior for due jobs, retry, cancellation, and action
  required states.

### Phase 4: Upload Adapters

- Ship YouTube upload, thumbnail, schedule, polling, and final URL recording.
- Implement TikTok upload/status once app permissions allow it.
- Implement Instagram media container/publish/status once permissions allow it.

### Phase 5: Team and Agency Controls

- Add workspace-level account ownership.
- Add role checks for connect, disconnect, schedule, cancel, and retry.
- Add account usage audit views.
- Add brand/channel defaults per connected account.

## Open Decisions

- Which server stack/database owns these tables in this repository once the web
  service is introduced.
- Whether desktop campaigns sync to the server at campaign creation or only
  when the user schedules a post.
- Which encryption provider is used in production.
- Whether first release supports user-owned accounts only or workspace-owned
  accounts as well.
- Which exact provider scopes and app-review paths will be requested for
  TikTok and Instagram.
