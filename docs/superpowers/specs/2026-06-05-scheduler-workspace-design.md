# Scheduler Workspace Design

## Summary

Awidat needs a product-facing scheduler workspace that makes the campaign
engine usable as a daily posting tool. The workspace should feel closer to an
OpusClip-style calendar and bulk scheduler than to a settings page: users pick
clips or long-form videos, select connected accounts, review generated
metadata, choose release timing, and then let the server-backed publishing
pipeline execute the posts.

This is not a separate scheduler for shorts versus long-form. Both content
types use one pipeline:

```text
SourceAsset
  -> PublishableItem
    -> PlatformVariant
      -> MetadataProfile
      -> ScheduleSlot
      -> PublishJob
```

Long-form YouTube episodes, YouTube Shorts, Instagram Reels, TikTok videos,
and podcast-derived clips differ in metadata shape, validation, and platform
fields. They do not need separate scheduling systems.

## Goals

- Add a first-class Calendar / Schedule workspace inside the product.
- Let users schedule one post or many posts from the same surface.
- Support long-form and short-form campaigns together.
- Generate platform-aware metadata: titles, descriptions, chapters, tags,
  captions, hashtags, thumbnails, and covers.
- Make bulk scheduling practical: start time, timezone, cadence, selected
  accounts, and per-post overrides.
- Show durable server job status: staged, scheduled, uploading, processing,
  published, failed, needs action, cancelled.
- Keep OAuth, token refresh, publish firing, retries, and final audit history
  server-owned.

## Non-Goals

- Do not build a detached generic social calendar that ignores Awidat's edit,
  render, transcript, and proof pipeline.
- Do not create separate scheduler implementations for podcasts, long-form
  YouTube, and shorts.
- Do not make the desktop frontend responsible for firing scheduled posts.
  The server-backed worker remains authoritative so posts can fire while the
  app is closed.
- Do not require every campaign to be auto-published. Approval and review
  remain first-class.

## Product Surface

### 1. Scheduler Workspace

The workspace is a main product destination, not only a modal inside Delivery.
It has three primary views:

- **Calendar** for month/week planning and scheduled/published status.
- **Queue** for dense review of upcoming jobs and failures.
- **Campaigns** for source-centered planning: one long-form asset with many
  derived posts.

The calendar should show post cards with platform icons, account name, title or
caption preview, status, and a final URL when published. Clicking a card opens
the post review drawer.

### 2. Source Picker

The Schedule action opens a source picker that can choose from:

- Current timeline render.
- Existing campaign manifest.
- Render queue item.
- Imported local video.
- Previously generated clip set.

The picker should support both "single video" and "clip collection" choices.
For a clip collection, the user can select one, several, or all clips before
continuing.

### 3. Post Review Drawer

Each selected post has a review surface with:

- Video preview and cover/thumbnail choice.
- Destination accounts and platform variants.
- Generated metadata with regenerate and manual edit actions.
- Platform validation warnings.
- Privacy/visibility and schedule settings.
- Approval state and publish job history.

The review surface should be the same for a single YouTube episode and for a
short-form clip. Only the metadata profile and platform fields change.

### 4. Bulk Schedule Modal

Bulk scheduling is a first-class flow for short-form campaigns. The user can
select many clips and set:

- Start date and time.
- Timezone.
- Cadence such as every 2 hours, daily, weekdays only, or custom interval.
- Target accounts and platforms.
- Default privacy/visibility.
- Whether to skip occupied slots.
- Whether to generate unique metadata per platform.

The preview must show the computed release sequence before scheduling. Users
can override individual slots without leaving the flow.

### 5. Metadata Generation Panel

Metadata generation is profile-driven. The user should be able to regenerate
for one post, one platform, or the full selected batch.

Examples:

- `youtube_long_form`: title, description, chapters, tags, thumbnail prompt,
  optional pinned comment.
- `youtube_short`: short title, caption, hashtags, cover timestamp.
- `instagram_reel`: caption, hashtags, cover, collaborator or mention fields
  later.
- `tiktok_short`: caption, hashtags, inbox/feed target, creator disclosure and
  eligibility fields.
- `podcast_episode_campaign`: long-form YouTube metadata plus derived shorts
  captions and staggered rollout slots.

The agent prompt should branch on content kind, platform, account rules, and
duration. It should not branch on separate scheduler code paths.

## Workflows

### Single Post Schedule

1. User opens Scheduler and clicks Schedule.
2. User selects one rendered video or timeline.
3. Awidat proposes metadata for selected platforms.
4. User selects accounts, visibility, and publish time.
5. Awidat validates each platform variant.
6. User approves.
7. Server creates scheduled publish jobs.
8. Calendar shows the post in the selected slot and later updates to published
   or failed.

### Bulk Shorts Campaign

1. User opens a generated clip set.
2. User selects several clips.
3. Awidat generates captions, hashtags, covers, and platform variants.
4. User chooses start time, timezone, and cadence.
5. Awidat previews computed slots across the calendar.
6. User approves the batch.
7. Server creates one publish job per platform variant.
8. Queue and calendar show every clip's status, retries, and final URLs.

### Long-Form Episode Campaign

1. User selects a podcast or long-form render.
2. Awidat generates a YouTube long-form package: title, description, chapters,
   tags, thumbnail, and visibility.
3. Awidat can also attach derived shorts from the same source.
4. User schedules the long-form release and optionally staggers related shorts.
5. The same publish job model handles both the episode and the shorts.

### Failure Recovery

1. A scheduled post fails or requires action.
2. Calendar and Queue show the normalized reason.
3. User opens the post review drawer.
4. User fixes the account, privacy, metadata, media constraint, or scheduled
   time.
5. User retries or reschedules.
6. The audit trail preserves the old failure and the new attempt.

## Data Model

The existing campaign manifest and server publishing domain remain the base.
The scheduler workspace should add client-side planning concepts that lower
into existing server rows.

### `ScheduleDraft`

A local editable plan before jobs are created:

- `draftId`
- `campaignId`
- `items`
- `selectedVariantIds`
- `batchRule`
- `timezone`
- `approvalState`
- `createdAt`
- `updatedAt`

### `ScheduleBatchRule`

The repeat pattern used to generate schedule slots:

- `startAt`
- `timezone`
- `intervalMinutes`
- `count`
- `skipOccupiedSlots`
- `allowedWeekdays`
- `accountIds`
- `platforms`

### `MetadataProfile`

The prompt and validation profile for platform-specific copy:

- `profileId`
- `contentKind`: `long_form`, `short`, `clip_set`, or `podcast_episode`
- `platform`
- `requiredFields`
- `optionalFields`
- `promptHints`
- `validationRules`

### Lowering to Server

When the user approves, Awidat maps each selected platform variant to the
server-backed social pipeline:

```text
ScheduleDraft
  -> campaign_variant_targets
  -> publish_jobs
  -> publish_job_events
```

The server owns final status, token refresh, retry state, provider IDs, final
URLs, and audit events.

## Platform and Privacy Rules

The UI must make platform constraints visible before scheduling:

- Missing connected account.
- Missing OAuth scope.
- Account not eligible for direct posting.
- Media duration or aspect ratio invalid.
- Thumbnail or cover missing where required.
- Privacy forced by platform audit, local configuration, or provider policy.
- Scheduled time invalid or too close to now.
- Upload quota or rate limit risk.

If a provider is forced private for audit or local config, the review drawer
must show that before approval. Users should never discover the privacy result
only after upload.

## Integration Points

- `UploadMetadataForm` remains useful for per-target metadata editing, but the
  scheduler workspace becomes the broader planning shell.
- `CampaignApprovalPanel` should evolve into or feed the post review drawer.
- `CampaignManifest` and `PlatformVariant` remain the handoff format from edit
  and render into publishing.
- `serverPublish.ts` remains the desktop bridge to the server-backed publish
  commands.
- `SocialSchedule`, `SocialJobs`, and `SocialAudit` should be folded into the
  scheduler workspace as panels or tabs instead of staying isolated demos.

## UX Principles

- Scheduling is the primary surface; connection status and job audit support
  it.
- Dense calendar and queue views are better than a marketing-style dashboard.
- Batch actions must always preview their effect before writing jobs.
- Generated metadata must be editable inline.
- Long-form and short-form posts should feel like siblings in the same
  campaign, not separate products.
- Published posts should expose the provider URL directly from the calendar,
  queue, and campaign history.

## Implementation Slices

### Slice 1: Workspace Shell and Read-Only Calendar

- Add Scheduler workspace navigation.
- Render existing server jobs and campaign variants in calendar and queue views.
- Open a post review drawer from a calendar card.
- No new scheduling writes yet.

### Slice 2: Single and Bulk Schedule Drafts

- Add `ScheduleDraft` and `ScheduleBatchRule` helpers.
- Support source selection from current render queue items and campaign
  manifests.
- Preview computed slots.
- Approve drafts into server-backed schedule jobs.

### Slice 3: Metadata Profiles

- Add metadata profile definitions for YouTube long-form, YouTube Shorts,
  Instagram Reels, TikTok, and podcast campaigns.
- Wire regenerate-one and regenerate-batch actions.
- Reuse existing target metadata storage where possible.

### Slice 4: Status, Recovery, and Audit Polish

- Merge schedule, jobs, and audit surfaces into one workflow.
- Add failure recovery actions: retry, reschedule, reconnect account, edit
  metadata, cancel.
- Show final provider URLs and immutable event history.

## Verification Strategy

- Unit-test schedule slot generation for timezone, interval, count, and
  skip-occupied behavior.
- Unit-test metadata profile selection for long-form versus short-form
  platform variants.
- UI-test bulk selection, slot preview, approval, and failure recovery states.
- Server integration tests should continue to cover publish job FSM, retries,
  token refresh, and final URL persistence.
- End-to-end local verification should use a short rendered asset and a real
  connected YouTube account, with privacy expectations shown before upload.
