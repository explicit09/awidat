---
name: yt-broll
description: Fetch B-roll from YouTube or Vimeo via yt-dlp. Higher-friction than stock-broll because the user is responsible for verifying rights — load this skill when the user explicitly asks for a YouTube or Vimeo clip by URL.
version: 0.1.0
tier: editorial
tools_allowlist:
  - download_yt_clip
  - apply_edl
  - inspect_moment
  - update_plan
---

# YouTube / Vimeo B-roll

You're fetching a clip from YouTube or Vimeo for use as B-roll. Unlike
Pexels (CC-licensed, attribution-only), YouTube/Vimeo content is
typically copyrighted and the user is responsible for verifying that
their use qualifies under fair use, has a license, or has explicit
permission from the rights-holder.

This skill is **not the default**. Prefer `stock-broll` (Pexels) for
generic cutaways. Load this skill only when:

- The user supplies a specific YouTube or Vimeo URL.
- The user explicitly asks for "that clip from YouTube" or similar.
- A creator-collab project where the rights chain is clear (e.g. the
  user's own past videos, or a guest's content the user has permission
  to use).

## The 3-step playbook

### 1. Walk the user through the rights situation

BEFORE you call `download_yt_clip`, explain to the user:

> "I can fetch that clip via yt-dlp. Before I do, you should know:
> YouTube content is copyrighted. Your use needs to either qualify
> under fair use (commentary, criticism, transformation), have an
> explicit license from the uploader, or be your own content. I'm
> not a lawyer; this is your call. Confirm you've assessed the rights
> situation and want me to proceed?"

Wait for an explicit confirmation. "Yes proceed" or equivalent. Do
NOT proceed if the user says "I think so" or "probably fine" —
ambiguity is a stop signal.

### 2. Download with the acknowledged flag

Once confirmed:

```
download_yt_clip(
  url="<the URL>",
  anchor={"transcript_snippet": "<the trigger phrase>"},
  duration_s=<typically 2.0–4.0>,
  source_start_s=<optional sub-window start>,
  source_end_s=<optional sub-window end>,
  position="overlay",
  acknowledged=true       # only after explicit user confirmation
)
```

If the source video is long (hours), use `source_start_s` and
`source_end_s` to download only the slice you need. Pulling a
60-minute video to use 4 seconds wastes bandwidth and disk.

The tool returns:
- `asset_path` (`raw/broll/yt-<hash>.mp4`)
- `edl_fragment` ready for apply_edl
- `downloads_remaining_this_session` (cap is 10)

The acknowledgment is recorded at `<project>/.awidat/yt_caveats.json`
so it persists across sessions.

### 3. Place via apply_edl

Hand the `edl_fragment` to `apply_edl` to actually place the cutaway
on the timeline. Same flow as `stock-broll`'s final step.

## When to refuse

- **User says "just download it"** without any rights discussion →
  refuse politely. Walk them through step 1 anyway — they may not
  realize the friction is intentional.
- **URL host isn't allowed** (the tool refuses anything not in the
  allowlist) → tell the user the supported hosts and stop.
- **Per-session download budget hit** → tell them, don't retry. They
  may want to restart the session.
- **yt-dlp not installed** → surface the install hint
  (`brew install yt-dlp` or `pipx install yt-dlp`) and stop.

## Common failure modes

- **"Video unavailable"**: yt-dlp returns non-zero. Surface the stderr
  excerpt; don't retry. Common reasons: private video, region-blocked,
  age-restricted, removed.
- **Format selection fails**: very rare; the tool requests
  `bv*[height<=1080][ext=mp4]+ba[ext=m4a]/...` which covers the typical
  YouTube tree. If it fails, the source is unusual (live stream,
  premiere, etc.) — ask the user for a different URL.
- **Sub-window doesn't match**: yt-dlp's `--download-sections` uses
  `*S-E` syntax with seconds. Make sure your `source_start_s` and
  `source_end_s` are in seconds, not minutes.

## You are done when...

- [ ] You explicitly walked the user through the rights situation
      and got explicit confirmation BEFORE setting `acknowledged=true`.
      ("Did the user understand?" is your check.)
- [ ] The download landed at `raw/broll/yt-<hash>.mp4` (the tool's
      response shows `downloaded: true` or `downloaded: false` for an
      idempotent re-fetch — both are fine).
- [ ] You handed the `edl_fragment` to `apply_edl` and `view_timeline`
      shows the cutaway at the expected anchor.
- [ ] The acknowledgment is in `<project>/.awidat/yt_caveats.json`
      (the tool persists this; you don't need to verify).

## Compared to stock-broll

| Dimension | `stock-broll` (Pexels) | `yt-broll` (yt-dlp) |
|---|---|---|
| License | CC, attribution-only | Mixed; user verifies |
| Friction | Low — agent picks freely from search | High — explicit caveat per URL |
| Best for | Generic cutaways ("city skyline") | Specific clips by URL |
| Rate limit | 200 searches/hr | 10 downloads/session |
| Cost | API key (free tier) | Bandwidth only |
| Failure mode | Quota exhausted | Source unavailable |

When in doubt, default to `stock-broll`. Use this skill only when the
user has explicitly asked for a third-party URL.
