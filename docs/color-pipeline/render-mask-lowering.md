# Render mask lowering

`montage.color_pipeline.mask_source` is no longer only reserved schema.
The timeline render path lowers the v1 masked look-LUT case into FFmpeg by
adding the mask as an extra input and compositing the masked grade over the
original clip.

## Implemented scope

- Single `look_lut` plus `mask_source` on `montage.color_pipeline`.
- Static-image masks, including grayscale or alpha-bearing images.
- Mask input is inserted after video/overlay inputs and before explicit
  audio-track inputs.
- Audio input labels are shifted past mask inputs, so masked video clips do
  not break explicit audio-track rendering.
- The effect registry advertises `montage.color_pipeline` as an FFmpeg-native
  backend because the render path now supports the color-management/LUT chain
  and this masked look-LUT subset.

Unsupported mask combinations remain explicit render limitations instead of
silent no-ops. Multi-stage color pipelines with masked IDT/shaper/ODT
combinations and complex transition/overlay interactions should continue to
surface `mask_in_complex_chain` or equivalent limitation records until they
have parity tests.

## Filter graph

Single clip, image mask, full-strength look LUT:

```text
[<seg_idx>:v:0]split[clip_orig][clip_to_lut];
[clip_to_lut]lut3d=file='<look>':interp=<interp>[clip_lutted];
[<mask_idx>:v:0]format=gray,loop=-1:1[mask_loop];
[clip_lutted][mask_loop]scale2ref=oh*mdar:ih[mask_scaled][clip_ref];
[clip_ref][mask_scaled]alphamerge[lut_alpha];
[clip_orig][lut_alpha]overlay=format=auto[<out>]
```

- `loop=-1:1` keeps a static mask alive for the segment duration.
- `scale2ref` matches the mask to the clip dimensions at render time.
- `alphamerge` puts the mask into the LUT-graded stream's alpha channel.
- `overlay=format=auto` composites the alpha-masked LUT result over the
  ungraded original.

## FFmpeg input list

`build_timeline_argv_full` and
`build_timeline_argv_with_audio_tracks_and_annotations` build inputs in this
order:

1. Timeline segments.
2. Video overlays.
3. Optional browser broadcast overlay.
4. Mask sources for segments with renderable masked look LUTs.
5. Explicit audio-track clips.

The mask index is derived from the count of segment, overlay, and broadcast
inputs plus the number of earlier masked segments:

```text
base_mask_index = segs.len()
               + video_overlays.len()
               + (browser_broadcast_overlay.is_some() as usize)
mask_idx(i) = base_mask_index + segments_with_mask_before(i)
```

Explicit audio-track inputs start after `mask_count`, which keeps audio labels
stable when masks are present.

## Tests

The render tests cover the current v1 behavior at argv/filtergraph level:

- Mask source is inserted as an additional FFmpeg input.
- Masked LUT emits `alphamerge` and `overlay=format=auto`.
- Audio input indices shift past mask inputs.
- Unsupported masked color-pipeline combinations produce limitations.

## Future

- Animated masks from video assets.
- Per-frame mask alpha modulation.
- Multiple masks per clip.
- Masked input/output transform LUT combinations.
- End-to-end masked-render fixture that runs FFmpeg on test media, not only
  argv/filtergraph assertions.
