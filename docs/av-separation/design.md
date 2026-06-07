# A/V Separation — Design Doc

**Status:** Proposal · **Branch:** `feat/av-separation` (worktree) · **Scope:** let editors silence/remove a clip's audio while keeping its picture (and vice-versa) — graph-native.

> **The bug, precisely:** today there is no way to edit a clip's audio independently of its picture. You can delete the whole clip (losing the image) or set its volume to 0 for the *entire* clip — but you cannot **remove the audio of a region while holding the picture**, mute one clip, or otherwise decouple sound from image. The desktop shows an audio lane, but it's the *video clip's muxed audio*, not an independently editable track — "two tracks that aren't tracking either thing."

> **Key finding (reframes the fix):** the **decoupled render machinery already exists and is exercised.** This is *not* a render rewrite — it's an authoring-grammar gap. See §1.

---

## 1. Verified current state

### The render has two A/V modes, auto-selected
`render_timeline` (`crates/render/src/timeline.rs`) picks the audio mode by whether any audio-track plans exist:

```
// timeline.rs:1908
if audio_tracks.is_empty() {
    audio_tracks = synthesize_split_edit_audio_tracks(&segs)?;
}
```

- **Coupled mode (default):** no audio tracks → the concat filter pulls each video clip's **muxed** audio alongside its picture (`...concat=n=N:v=1:a=1`). Module doc, `timeline.rs:15`: *"most montage projects keep video and audio paired in the same source file."* Here audio is locked to picture per clip — **the source of the bug.**
- **Decoupled mode:** any audio track present → video tracks render **video-only** (`plan_video_only_filter` → `concat=…:v=1:a=0[vonly]`, `timeline.rs:9768`) and **all** audio is mixed from audio-track plans via `amix` (`plan_audio_mix_filter`, `:9936`). Picture and sound are fully independent.

### The decoupled mode is already produced automatically — for split edits
`synthesize_split_edit_audio_tracks` (`timeline.rs:3127`) is the template we extend:
- If **any** segment has a split-edit handle (`audio_lead_s`/`audio_trail_s` > 0), it synthesizes **one audio track per segment covering the whole timeline** — each segment's audio pulled from its own source at `source_start_s`, shifted by the lead/trail, with `Gap` items for spacing.
- Render then flips to video-only + amix. Covered by `split_edit_audio_tracks_use_video_only_render_path` (`:12712`).

So the engine **can already** render picture and sound independently; the J/L (`split-edit-director`) path is living proof. What's missing is a way for the agent to say *"mute this clip"* or *"remove audio here, keep the picture"* and have synthesis honor it.

### What exists vs. what's missing (authoring)
- ✅ Decoupled render path + per-segment audio synthesis (above).
- ✅ Explicit audio-kind tracks: `collect_audio_track_plan` (`:2962`), `parse_audio_track_settings` (`:3224`); `AudioTrackPlan { muted, solo, ducking, … }`.
- ✅ `Set Volume` (clip/track level), `Set Audio Lead/Trail` (J/L), track-level audio FX, `link_group_id` (links a video clip ↔ a separate audio clip), `AudioRelation { Sync, AudioLeads, AudioTrails, Overlap, AudioCut }`, `Clip.active: bool`.
- ❌ **No op to silence/remove a clip's audio while keeping picture.** Verified: no `Mute`, `Remove Audio`, `Detach`, `Silence`, `Unlink` in `edl/op.rs` or `edl/parser.rs`. Only `Set Volume` (whole-clip gain — and in coupled mode the stream is still muxed/present).

---

## 2. The gap

A clip's audio is editable **only as a whole-clip volume**, and only the muxed stream. There is no grammar for:
- **G-S1 — Mute a clip** (audio off, picture held).
- **G-S2 — Remove audio over a region** `[t0,t1]` while holding the picture (the user's exact ask).
- **G-S3 — (deferred) independently reposition** a clip's audio relative to its picture beyond the existing J/L lead/trail.

These need authoring ops + a per-clip audio representation that the *existing* synthesis path honors. No new render path.

---

## 3. Design

### Principle
Reuse the decoupled path. Add a **clip-level audio override** in metadata; make the synthesis trigger and loop honor it. When any clip carries an override, the whole timeline renders decoupled (video-only + per-segment audio), exactly like split edits today — so audio elsewhere is preserved, and the overridden clip's audio is silenced/cut while its picture stays on the video track.

### 3.1 Data model — clip audio override
Add to `MontageClipMetadata` (`crates/proto/src/montage_meta.rs`) a typed, optional field (forward-compatible; absent = today's coupled behavior):

```rust
pub struct ClipAudioOverride {
    /// Whole-clip silence; picture is kept.
    pub muted: bool,
    /// Clip-local source-time spans to silence, picture kept. Each (start_s, end_s).
    pub removed_ranges: Vec<(f64, f64)>,
}
```

Carried on the OTIO clip; read when building `TimelineSegment`. (Alternative considered: a dedicated `montage.audio_override` effect like `montage.sync_group`. Rejected — audio mute/removal is a property of the clip, not a stackable effect, and J/L already lives in clip metadata via `split_edit`. Keep it with `split_edit`/`audio_relation`.)

### 3.2 Render — extend synthesis (no new path)
`TimelineSegment` gains `audio_muted: bool` and `audio_removed_ranges: Vec<(f64,f64)>`, populated from the clip override during segment building.

1. **Trigger:** generalize `segment_has_split_edit_audio` → `segment_needs_synthesized_audio` = split-edit **or** `audio_muted` **or** non-empty `audio_removed_ranges`. (Keeps the existing `audio_tracks.is_empty()` gate at `:1908`.)
2. **Synthesis loop** (`synthesize_split_edit_audio_tracks`, renamed `synthesize_audio_tracks`):
   - `audio_muted` → emit a `Gap` for that segment's audio slot instead of the `AudioClipPlan` (picture stays; sound gone).
   - `audio_removed_ranges` → split the segment's `AudioClipPlan` into sub-clips around each removed span, inserting `Gap`s for the silenced spans (clip-local → source-time mapping mirrors `source_start_s` math already there).
   - Otherwise unchanged (full-audio clip), so non-overridden clips keep sound when the timeline flips decoupled.
3. Coupled mode is untouched when no overrides and no split edits exist — zero behavior change for existing projects.

### 3.3 Authoring — new EDL ops
In `edl/op.rs` + `edl/parser.rs` + `edl/apply.rs`, mirroring the `Set Volume` / `Set Audio Lead` shape (anchor by `clip_uuid`/`transcript_snippet`):

- **`Mute Clip`** — `@@ anchor` + `+ muted: true|false`. Sets `ClipAudioOverride.muted`.
- **`Remove Audio`** — `@@ anchor` + `+ start_s` + `+ end_s` (clip-local). Appends to `removed_ranges` (merges overlaps). A companion `+ clear: true` resets ranges. Picture untouched.

Both validate finiteness/ordering and that ranges fall within the clip, failing before mutation (same discipline as `apply_multicam_plan`).

### 3.4 Agent surface
Mirror the ops into `montage_mcp` if a direct tool is wanted; at minimum they're reachable through `apply_edl`. Update the `split-edit-director` skill (or a small `audio-editor` skill) to mention `Mute Clip` / `Remove Audio` and the "picture is held" guarantee. Classify any new read-only helper as evidence in `capability_metadata.rs` (consistency with the multicam G5 pass).

### 3.5 "Detach" semantics
True NLE "detach audio to its own movable track" (G-S3) is **deferred**. With this design, the audio is already conceptually independent (synthesized per-segment); muting/removing covers the high-value cases. Independent *repositioning* beyond J/L lead/trail is a phase-2 extension (a clip-local audio offset field honored by the same synthesis loop).

---

## 4. Phasing

> **Status (2026-05-30): Phases 1–3 shipped on `feat/av-separation`.** ClipAudioOverride{muted, removed_ranges} on the clip; render synthesizes per-segment audio (silence for mutes, kept-clip/gap splits for removed ranges) via the existing decoupled video-only+amix path; `Mute Clip` and `Remove Audio` EDL ops (parse + apply); apply_edl grammar + `audio-separation` skill for discoverability. Unsupported combos (speed/split-edit + removal) fail loud via `RenderTimelineError::AudioRemovalUnsupported`. Verified: proto 114, core 815, render 272, skill_catalog 13 — all pass; fmt + clippy clean.

1. ✅ **Phase 1 — Mute Clip** (G-S1): data model + segment field + synthesis gap + `Mute Clip` op + apply + tests. *Done.*
2. ✅ **Phase 2 — Remove Audio region** (G-S2): `removed_ranges` + range-splitting in synthesis + `Remove Audio` op + tests. *Done — delivers the user's exact scenario.*
3. ✅ **Phase 3 — surface + skill**: apply_edl grammar entries + `audio-separation` skill + catalog/description tests. *Done. Per-tool capability metadata is N/A — these are EDL ops, not standalone tools.*
4. **Deferred** — independent audio repositioning (G-S3); audio removal combined with speed/split-edit on one clip; desktop audio-lane edit affordance.

Each phase: build + targeted tests + clippy green, committed as a strategic unit.

## 5. Tests
- Render: `muted clip emits video-only concat + audio gap` (assert `v=1:a=0[vonly]` and the muted segment contributes silence); `removed range splits audio with gaps`; `no override + no split edit stays coupled` (`v=1:a=1`, regression guard).
- Apply: `Mute Clip` stamps/clears override; `Remove Audio` appends/merges ranges; invalid range rejected before mutation.
- End-to-end: a 2-clip timeline, mute clip 1 → clip 1 picture present, silent; clip 2 audio intact.

## 6. Cross-links (A/V sync follow-ons, separate but adjacent)
From the multicam audit (`docs/multicam/design.md`):
- **Render ignores `montage.sync_group`** — sync offset is realized only by `apply_set_sync_group` repositioning the clip; **negative offsets clamp to 0** (`offset_s.max(0.0)`).
- **G1b — multicam source-range ignores offset:** `build_multicam_program_track` uses program time as the source-media start, so separate-device cameras show frames off by their offset. Small follow-on; not part of this branch unless folded in.

## 7. Open questions
1. **Range coordinate space** — clip-local source seconds (proposed) vs timeline seconds. Clip-local is robust to moves/trims; the agent gets timeline times, so the op should accept timeline times and convert at apply using the clip's source_range. *(Lean: accept timeline times, store clip-local.)*
2. **Linked audio (`link_group_id`)** — when a clip already has a linked audio sibling (J/L), does `Mute Clip` target the sibling or the muxed audio? *(Lean: override always governs the synthesized audio for that source clip; siblings are a separate concern.)*
3. **`Set Volume 0` vs `Mute Clip`** — keep both: volume is a mixable level (still in the mux in coupled mode); mute forces decoupled + true silence with picture held. Document the difference.
