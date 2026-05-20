# Render mask lowering — design

`awidat.color_pipeline.mask_source` is reserved schema (Stage 9). Apply-time
validation accepts the slot and stamps the path on the OTIO; `read_color_pipeline`
populates `ColorPipelinePlan.mask_source: Option<PathBuf>`; render surfaces a
`mask_not_implemented` `RenderPlanLimitation` per masked clip so the agent
sees its regional-grading request was a no-op. This memo captures the design
for closing the loop.

## Why deferred

Unlike auto-zscale or the catalog work, regional grading touches:

1. The FFmpeg argv input list — the mask becomes a separate `-i`.
2. `FilterPlanner`'s segment→input-index assignment.
3. A multi-input filter pattern (`alphamerge` + `scale2ref` + `overlay`).
4. Audio-input index reshuffling in `build_timeline_argv_with_audio_tracks`.

None of the in-tree integration tests run ffmpeg on a graded frame, so a
half-shipped chain wouldn't have a regression net. Better to land this with
a real masked-render fixture than to guess.

## Filter graph (single clip, image mask, full-strength look LUT)

```
[<seg_idx>:v:0]split[clip_orig][clip_to_lut];
[clip_to_lut]lut3d=file='<look>':interp=<interp>[clip_lutted];
[<mask_idx>:v:0]format=gray,loop=-1:1[mask_loop];
[clip_lutted][mask_loop]scale2ref=oh*mdar:ih[mask_scaled][clip_ref];
[clip_ref][mask_scaled]alphamerge[lut_alpha];
[clip_orig][lut_alpha]overlay=format=auto[<out>]
```

- `loop=-1:1` keeps the mask alive for the segment's duration when the
  source is a static image.
- `scale2ref` matches the mask to the clip's dimensions on the fly so the
  mask author doesn't need to pre-resize.
- `alphamerge` puts the mask into the LUT-graded stream's alpha channel.
- `overlay=format=auto` composites the alpha-masked LUT result over the
  un-graded original.

## FFmpeg input list plumbing

Today `build_timeline_argv_full` and `build_timeline_argv_with_audio_tracks`
build the `-i` list in this order:

1. `segs` — one `-i` per segment.
2. `video_overlays` — one `-i` per overlay.
3. `browser_broadcast_overlay` — optional, single `-i`.
4. `audio_tracks` — one `-i` per audio clip (only in `_with_audio_tracks`).

Masks fit between (3) and (4):

```
base_mask_index = segs.len()
               + video_overlays.len()
               + (browser_broadcast_overlay.is_some() as usize)
mask_idx(i) = base_mask_index + segments_with_mask_before(i)
```

Audio-track input indices must shift past `mask_count` whenever masks are
present. The existing audio-input loop assumes audio starts right after the
broadcast overlay, so this is the one cross-cutting edit.

## `FilterPlanner` integration

`stage_segment_inputs` and `stage_segment_video_input` need the mask input
label for each segment. Simplest: add `mask_input_index: Option<usize>` to
`TimelineSegment`, populated when masks are added to the input list. Staging
functions check `seg.mask_input_index` and route through the masked block
instead of the unmasked `color_pipeline_filter_block`.

## Refactor of `color_pipeline_filter_block`

The block currently emits the LUT chain as one labeled fragment. To support
masking we either:

- **(a)** add a `mask_input_label: Option<&str>` parameter and weave the
  alphamerge/overlay subchain in when set, OR
- **(b)** add a sibling `masked_color_pipeline_filter_block` that takes the
  mask label.

Option (a) keeps one source of truth but the function gets messy when
`look_strength<1` (split/blend already forks the stream). Option (b) keeps
the un-masked case clean — and masked cases probably want to ignore
`look_strength<1` for v1, since the mask is its own opacity-like control.
Recommend **(b)** initially.

## Supported scope for v1

- Single `look_lut` + `mask_source`. No `input_transform_lut` / `shaper_lut`
  / `output_transform_lut` combined with masking yet.
- Static-image masks (PNG with alpha, or grayscale image). `loop=-1:1`
  keeps the frame live for the segment's duration.
- `look_strength` is ignored when `mask_source` is set — the mask carries
  the regional opacity.
- No transitions / video overlays interacting with the masked stream
  (FilterPlanner v2 work).

Cases outside this scope keep the existing `mask_not_implemented`
limitation. Surface a tighter `mask_in_complex_chain` kind for the
unsupported-combination case so the agent knows the gap is "this combo,"
not "masks at all."

## Tests to add (argv-level, no ffmpeg)

- `mask_source_inserted_as_additional_input` — argv contains `-i <mask_path>`
  after video overlays.
- `masked_lut_emits_alphamerge_and_overlay` — filter_complex string contains
  the `alphamerge` and `overlay=format=auto` clauses.
- `audio_input_indices_shift_past_masks` — audio-track filter labels stay
  consistent when masks are present.

## Future

- Animated masks (video files instead of static images).
- Per-frame mask alpha modulation (envelope animations on the mask track).
- Multiple masks per clip stacking (face + sky, for example).
- Masked output LUTs — currently the design only masks the look LUT; the
  `output_transform_lut` stays outside the masked branch.
- Real masked-render fixture under `crates/render/tests/` driving ffmpeg
  end-to-end so the chain can be regression-tested.
