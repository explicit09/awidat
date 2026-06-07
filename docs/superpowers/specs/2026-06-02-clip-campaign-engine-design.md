# Clip Campaign Engine Design

## Summary

Montage should compete with Opus Clip by treating publishing as a campaign
engine, not a standalone scheduler. The product creates publishable clips and
long-form posts from a source asset, packages each post with platform-specific
metadata and proof, then schedules and publishes through connected user
accounts.

The first product target is the Clip Campaign Engine: a shared pipeline that
supports both podcast episodes and arbitrary uploaded videos. Podcast campaigns
produce long-form YouTube posts plus shorts. Shorts campaigns produce one or
more short-form posts from any source video. Both campaign types lower into the
same publish package, approval, scheduling, OAuth, and platform adapter model.

## Goals

- Generate full campaign plans from source media, not isolated clips.
- Support podcast long-form delivery and shorts-from-any-upload in one model.
- Let users connect their own social accounts through OAuth.
- Let users approve, edit, schedule, and publish generated posts.
- Keep platform-specific rules isolated behind adapters.
- Preserve Montage's proof-oriented workflow: every publishable item should know
  which render, transcript anchors, metadata, and preflight checks support it.

## Non-Goals

- Do not build a generic Buffer clone detached from Montage's editing pipeline.
- Do not attempt fully unattended autopilot before approval and audit history
  exist.
- Do not make the desktop app responsible for reliable scheduled posting.
  Scheduling needs a server-side worker.
- Do not assume every platform supports native scheduled publishing. The queue
  must be able to hold jobs and publish at the target time itself.

## Core Concept

The core object is a campaign:

```text
SourceAsset
  -> CampaignPlan
    -> PublishableItem
      -> PlatformVariant
        -> PublishJob
```

A campaign is the user's rollout plan for a source asset. It may include one
long-form post, many short-form posts, or both.

A publishable item is a platform-agnostic post candidate. It contains the
rendered artifact, title or caption, tags, cover or thumbnail, transcript
anchors, duration, aspect ratio, evidence, and approval state.

A platform variant adapts one publishable item to a specific destination such as
YouTube, YouTube Shorts, TikTok, Instagram Reels, or Instagram feed.

A publish job is the scheduled execution record for a platform variant. It owns
timing, retries, platform IDs, final URLs, failure reasons, and audit history.

## Campaign Templates

### Podcast Campaign

Input: a full podcast or interview episode.

Expected outputs:

- Long-form YouTube render.
- YouTube title, description, chapters, tags, and thumbnail candidates.
- 5-20 short-form clips selected from hooks, strong answers, debates, or useful
  standalone moments.
- Vertical and square derivatives as needed.
- Platform captions, hashtags, cover timestamps, and schedule slots.
- A staggered rollout calendar, for example a long-form release followed by
  shorts over one or two weeks.

### Shorts Campaign

Input: any uploaded video.

Expected outputs:

- One or more short-form clips.
- Hook-focused captions and hashtag variants.
- Vertical or square renders based on selected targets.
- Platform-specific settings for TikTok, Reels, and Shorts.
- Schedule slots that can be approved individually or as a batch.

## Publish Package Manifest

Montage should write a durable manifest for every campaign. The manifest is the
handoff contract between editing, approval, and posting.

Minimum fields:

- `campaign_id`
- `source_asset_id`
- `campaign_type`: `podcast` or `shorts`
- `items`
- `platform_variants`
- `schedule`
- `approval_state`
- `evidence`
- `created_at`
- `updated_at`

Each publishable item should include:

- `item_id`
- `kind`: `long_form`, `short`, `thumbnail`, `caption`, or `metadata`
- `title`
- `description` or `caption`
- `hashtags`
- `artifact_path`
- `thumbnail_path` or `cover_timestamp_ms`
- `duration_s`
- `aspect_ratio`
- `transcript_anchors`
- `preflight_checks`
- `approval_state`

Each platform variant should include:

- `variant_id`
- `item_id`
- `platform`
- `account_id`
- `platform_fields`
- `scheduled_for`
- `status`
- `publish_job_id`

## OAuth Account Registry

Connected accounts live outside project files because they belong to users and
workspaces, not media projects. The registry stores:

- User/workspace owner.
- Platform name.
- Platform account ID and display name.
- OAuth scopes granted.
- Token reference or encrypted token material.
- Token expiry and refresh state.
- Permission/audit status.
- Last successful publish and last failure.

The desktop app can start OAuth, but final token storage and scheduled posting
should live in a server service.

## Publishing Queue

Scheduled posting needs a durable server-side queue. The queue should support:

- Exact scheduled time.
- Idempotency key per platform variant.
- Retry policy by platform error type.
- Manual retry after user fixes a platform setting.
- `requires_action` state for expired tokens, missing permissions, review
  requirements, or platform-specific blocks.
- Immutable event history for audit and support.

Publish job states:

```text
draft
approved
scheduled
uploading
processing
published
failed
requires_action
cancelled
```

## Platform Adapters

Adapters isolate platform-specific OAuth, validation, upload, publishing, and
status polling.

Shared adapter interface:

- `validate_variant(variant, account)`
- `prepare_upload(variant, account)`
- `publish_now(variant, account)`
- `poll_status(job)`
- `normalize_error(error)`
- `fetch_metrics(post_id)`

Initial adapters:

- YouTube: long-form and Shorts uploads, metadata, thumbnail, privacy, and
  scheduled publication through `publishAt` where available.
- TikTok: Direct Post flow, creator info, privacy options, explicit user
  consent, upload/status polling, and app audit handling.
- Instagram: professional-account publishing flow, media container creation,
  media publish, status polling, and Reels/feed distinctions.

Platform API limitations are product constraints. The UI should show whether a
platform supports native scheduling, queue-based scheduling, public posting,
manual review, or requires account/action fixes.

## Approval UI

The user-facing approval surface should be campaign-native:

- Campaign overview with total posts, target platforms, and date range.
- Calendar or queue view for scheduled posts.
- Per-post preview with video, cover, caption, hashtags, target account, and
  platform warnings.
- Batch approve, individual approve, edit, reschedule, cancel, and retry.
- Clear status after publishing, including final URLs and failure reasons.

The UI should make the first workflow feel like:

> Turn this episode into a two-week campaign.

The user should not have to manually assemble platform posts one by one unless
they choose to.

## Data Flow

1. User imports or selects a source asset.
2. Montage indexes and analyzes media.
3. User chooses `Podcast Campaign` or `Shorts Campaign`.
4. Campaign planner proposes publishable items with evidence.
5. Renderer creates required long-form, vertical, square, caption, and thumbnail
   artifacts.
6. Preflight validates artifacts against target platform constraints.
7. Manifest is written and shown in the approval UI.
8. User connects OAuth accounts if needed.
9. User approves or edits campaign posts.
10. Server queue schedules publish jobs.
11. Platform adapters publish, poll, retry, and record platform IDs/URLs.
12. Metrics are fetched later and attached to the campaign.

## Error Handling

Errors should be normalized into user-actionable categories:

- `token_expired`
- `missing_scope`
- `platform_app_not_approved`
- `account_not_eligible`
- `media_constraint_failed`
- `rate_limited`
- `daily_post_cap_reached`
- `platform_processing_failed`
- `network_or_server_error`
- `manual_review_required`

Each failed job should preserve the original platform error, normalized error,
recommended fix, and whether retry is automatic or user-triggered.

## Rollout Phases

### Phase 1: Campaign Manifest and Local Approval

- Add campaign manifest format.
- Generate podcast and shorts campaign plans from existing Montage outputs.
- Show a local approval/queue surface without live posting.
- Validate render artifacts and metadata against platform profiles.

### Phase 2: Server Queue and YouTube Adapter

- Add server-side user/workspace account registry.
- Add OAuth connection flow.
- Add durable scheduled jobs.
- Implement YouTube upload/schedule/status for long-form and Shorts.

### Phase 3: TikTok and Instagram Adapters

- Add TikTok Direct Post support with creator info and explicit consent.
- Add Instagram professional-account publishing.
- Add adapter-specific preflight warnings and failure states.

### Phase 4: Campaign Intelligence

- Improve clip selection, caption variants, hashtag strategy, and schedule
  spacing.
- Add performance feedback from published posts.
- Let users create reusable brand/channel rules.

### Phase 5: Controlled Autopilot

- Allow approved workspaces to auto-generate campaigns from new uploads.
- Keep policy gates for account permission, content labeling, and final approval
  thresholds.
- Add guardrails for spam limits, duplicate content, and low-confidence clips.

## Testing Strategy

- Unit-test manifest serialization and migration.
- Unit-test platform validation without hitting live APIs.
- Add adapter contract tests with mocked platform responses.
- Add queue idempotency and retry tests.
- Add UI tests for approve, reschedule, failed, requires-action, and published
  states.
- For live integrations, use explicit sandbox/test accounts and never rely on
  real scheduled posting in ordinary CI.

## Open Decisions

- Whether the campaign manifest should live inside the local Montage project
  only, or be synced to a server as soon as it is created.
- Whether the first approval UI should be inside the desktop app, a web app, or
  both.
- Whether team/workspace permissions are required in the first server version.
- Which exact API/version and review path should be used for each platform at
  implementation time.
