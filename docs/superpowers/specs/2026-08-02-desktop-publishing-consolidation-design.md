# Desktop Publishing Consolidation Design

**Date:** 2026-08-02

## Decision

Keep one publishing execution path: the server-backed `montage-social` / `montage-social-server` path already used by the production UI. Remove the dormant desktop-local provider registry, OAuth/keychain store, upload queue, provider adapters, and their IPC commands.

Retain AI disclosure detection as a small read-only desktop capability because it inspects the currently loaded local project and feeds visible disclosure UI. It will no longer mutate an unused local upload queue.

## Evidence

- Production campaign publishing calls `publishCampaignViaServer`.
- Render completion and retry call `publishRenderTargetsViaServer`.
- Scheduler and social-account surfaces call `social_*` commands backed by `SocialClient`.
- The local commands for listing providers, OAuth, direct upload, upload queue lifecycle, credentials, and disclosure rehydration have no production frontend callers.
- `startCampaignUploads`, the only frontend function that drives the local queue, is called only by tests.
- Upload metadata and target preferences are already held by frontend stores and consumed directly by the server publisher. Their best-effort local-backend mirrors are redundant.

## Resulting boundary

```text
local project -> compute_ai_disclosure -> disclosure UI
render/campaign/scheduler -> social_* IPC -> SocialClient -> montage-social-server -> providers
```

There is no local fallback today, so deleting the uncalled implementation does not remove an active failover path.

## Changes

1. Reduce `commands/publishing.rs` to `compute_ai_disclosure`.
2. Reduce `publishing/mod.rs` to the AI-disclosure module and delete local provider/upload modules.
3. Remove `UploadQueue` from `MontageState` and unregister dead IPC commands.
4. Make upload metadata and target preferences localStorage-only.
5. Remove App bootstrap hydration for deleted backend preferences.
6. Delete test-only `startCampaignUploads` and its local-queue tests; retain campaign request and server-publishing tests.
7. Remove desktop dependencies used only by the deleted provider stack.
8. Update comments that still describe the retired dispatcher.

## Preserved contracts

- Connected-account management and publishing remain server-backed.
- Render-done auto-publish, campaign publish, retry, scheduler, and job polling keep their current `social_*` path.
- Per-target metadata and default target preferences survive reload through localStorage.
- AI disclosure detection, credits, UI chips/banner, and the auto-disclose preference remain.
- No generated protocol files or user-owned dirty files are touched.

## Verification

- Frontend publisher, metadata, preference, disclosure, and social tests.
- Frontend typecheck.
- `cargo test -p montage-desktop` and `cargo clippy -p montage-desktop --all-targets -- -D warnings`.
- Workspace compile check if the focused gates pass.
- Static proof that deleted commands and modules have no remaining references.

