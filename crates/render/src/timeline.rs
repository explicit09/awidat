//! Timeline-render planning: walk a project's OTIO and produce the
//! [`RenderJobSpec`] that ffmpeg will execute.
//!
//! Extracted from `awidat-core::tools::start_render` so the desktop's
//! Export button can call into the same logic without going through
//! the agent tool. Both call sites depend on this module; both
//! produce identical specs.
//!
//! The non-obvious bit is the **single re-encode at concat
//! boundaries**. Stream-copy concat at non-keyframe-aligned cut
//! points produces audible clicks (DTS-seam scratch); we accept
//! the encoder cost to avoid that.
//!
//! Audio tracks aren't enumerated separately — most awidat
//! projects keep video and audio paired in the same source file,
//! so the concat filter pulls each input's audio stream alongside
//! its video stream.
//!
//! Gaps and Transitions are skipped in v1; they land when the EDL
//! grows transition / gap awareness on the render side.

use std::path::{Path, PathBuf};

use awidat_proto::otio::{MediaReference, StackChild, TrackChild, TrackKind};
use awidat_proto::project::{files, read_otio_timeline};
use chrono::Utc;
use thiserror::Error;

use crate::job::RenderJobSpec;

/// Errors building a timeline-render spec.
#[derive(Debug, Error)]
pub enum RenderTimelineError {
    /// `<project_root>/project.otio.json` doesn't exist.
    #[error("no project.otio.json found at {0} — this isn't an awidat project root")]
    NoOtio(PathBuf),
    /// OTIO file present but parse / validation failed.
    #[error("timeline parse failed: {message}")]
    OtioParse {
        /// Diagnostic from the underlying parser.
        message: String,
    },
    /// A clip referenced an asset path that isn't on disk.
    #[error("timeline references missing asset {missing} (clip '{clip_name}')")]
    MissingAsset {
        /// Clip name from the OTIO.
        clip_name: String,
        /// Absolute path that didn't resolve.
        missing: PathBuf,
    },
    /// A clip lacked a `source_range`.
    #[error("clip '{clip_name}' has no source_range — can't extract a renderable segment")]
    ClipMissingRange {
        /// Clip name from the OTIO.
        clip_name: String,
    },
    /// The timeline parsed but has no clips on any video track.
    #[error("timeline has no clips to render")]
    EmptyTimeline,
}

/// One source-media segment to feed into the timeline-render concat.
/// Public so callers can sum durations or otherwise inspect the plan
/// before kicking off ffmpeg.
#[derive(Debug, Clone, Default)]
pub struct TimelineSegment {
    /// Absolute path to the source media.
    pub asset_path: PathBuf,
    /// Seconds into the source media where the cut starts.
    pub start_s: f64,
    /// Seconds of duration to take from the source.
    pub duration_s: f64,
    /// Linear gain multiplier for this segment's audio. `None` means
    /// no `awidat.volume` effect is on the underlying clip — the
    /// FilterPlanner skips emitting a `volume=` filter and the audio
    /// passes through unchanged. `Some(1.0)` is unity (functionally
    /// identical to `None` but the planner still emits the filter
    /// for explicitness).
    pub volume: Option<f64>,
    /// Playback rate multiplier. `None` means no `awidat.speed`
    /// effect — the segment plays at 1×. The segment's contribution
    /// to the master timeline duration is `duration_s / factor` when
    /// `factor` is `Some`. (Step 15.4 wires the setpts/atempo path;
    /// 15.3 only plumbs the field.)
    pub speed: Option<f64>,
}

/// Pull a numeric metadata field off the first effect on `clip` whose
/// `effect_name` matches. Returns `None` when no such effect exists
/// or the metadata field is missing / non-numeric. Used to surface
/// awidat.volume / awidat.speed values into the render pipeline.
fn read_effect_number(
    clip: &awidat_proto::otio::Clip,
    effect_name: &str,
    field: &str,
) -> Option<f64> {
    clip.effects
        .iter()
        .find(|e| e.effect_name == effect_name)
        .and_then(|e| e.metadata.get(field))
        .and_then(|v| v.as_f64())
}

/// Walk `<project_root>/project.otio.json` and collect every
/// video-track clip's `(asset, source_range)` in playback order.
/// Skips Gap, Transition, and nested Stack children for v1.
///
/// Wraps [`collect_timeline_plan`] and drops the transitions —
/// preserved for callers that don't need transition awareness.
pub fn collect_timeline_segments(
    project_root: &Path,
) -> Result<Vec<TimelineSegment>, RenderTimelineError> {
    let (segs, _) = collect_timeline_plan(project_root)?;
    Ok(segs)
}

/// Walk `<project_root>/project.otio.json` and collect both the
/// renderable segments AND the transitions between adjacent
/// segments. Returned in playback order; `TransitionPlan` indices
/// reference the returned segments slice.
///
/// Step 14.5: the render pipeline uses this to splice xfade filters
/// between the segments that have a [`TrackChild::Transition`]
/// between them on the OTIO track.
pub fn collect_timeline_plan(
    project_root: &Path,
) -> Result<(Vec<TimelineSegment>, Vec<TransitionPlan>), RenderTimelineError> {
    let otio_path = project_root.join(files::OTIO);
    if !otio_path.exists() {
        return Err(RenderTimelineError::NoOtio(otio_path));
    }
    let mut warnings = Vec::new();
    let timeline = read_otio_timeline(&otio_path, &mut warnings).map_err(|e| {
        RenderTimelineError::OtioParse {
            message: e.to_string(),
        }
    })?;

    let mut segs = Vec::new();
    let mut transitions = Vec::new();
    for child in &timeline.tracks.children {
        let StackChild::Track(track) = child else { continue };
        if !matches!(track.kind, TrackKind::Video) {
            continue;
        }
        // Walk the track's children. Clips become segments; a
        // Transition immediately following a Clip queues a transition
        // pointing at the *next* clip we'll see (`pending_transition`).
        // Other children (Gap, Stack) reset the pending state — they
        // can't sit between clips that share a transition in v1.
        let mut pending_transition: Option<(String, f64)> = None;
        for tc in &track.children {
            match tc {
                TrackChild::Clip(clip) => {
                    let MediaReference::External(ext) = &clip.media_reference else {
                        pending_transition = None;
                        continue;
                    };
                    let Some(range) = clip.source_range.as_ref() else {
                        return Err(RenderTimelineError::ClipMissingRange {
                            clip_name: clip.name.clone(),
                        });
                    };
                    let asset_path = project_root.join(&ext.target_url);
                    if !asset_path.exists() {
                        return Err(RenderTimelineError::MissingAsset {
                            clip_name: clip.name.clone(),
                            missing: asset_path,
                        });
                    }
                    let volume = read_effect_number(clip, "awidat.volume", "value");
                    let speed = read_effect_number(clip, "awidat.speed", "factor");
                    let new_index = segs.len();
                    segs.push(TimelineSegment {
                        asset_path,
                        start_s: range.start_time.to_seconds(),
                        duration_s: range.duration.to_seconds(),
                        volume,
                        speed,
                    });
                    if let Some((kind, duration_s)) = pending_transition.take()
                        && new_index > 0
                    {
                        transitions.push(TransitionPlan {
                            from_segment_index: new_index - 1,
                            to_segment_index: new_index,
                            kind,
                            duration_s,
                        });
                    }
                }
                TrackChild::Transition(t) => {
                    let total = t.in_offset.to_seconds() + t.out_offset.to_seconds();
                    pending_transition = Some((t.transition_type.clone(), total));
                }
                TrackChild::Gap(_) | TrackChild::Stack(_) => {
                    pending_transition = None;
                }
            }
        }
    }
    Ok((segs, transitions))
}

/// One transition between two segments in the timeline. Step 14.4
/// introduces this type so the [`FilterPlanner`] has a slot for
/// future transition wiring; in 14.4 callers always pass an empty
/// `transitions` slice and the planner emits the same monolithic
/// concat filter as before.
///
/// `from_segment_index` and `to_segment_index` are indices into the
/// segments slice the planner is fed. They MUST be adjacent
/// (`to == from + 1`); the apply layer rejects non-adjacent
/// transitions before they reach here.
#[derive(Debug, Clone)]
pub struct TransitionPlan {
    /// Index of the outgoing segment.
    pub from_segment_index: usize,
    /// Index of the incoming segment. Must be `from_segment_index + 1`.
    pub to_segment_index: usize,
    /// Transition kind (`"SMPTE_Dissolve"`, `"awidat.fade_in"`, etc).
    /// Wired to ffmpeg's xfade transition names in 14.5.
    pub kind: String,
    /// Total transition duration on the timeline, in seconds.
    pub duration_s: f64,
}

/// Plans the `-filter_complex` argument + map labels for a render.
///
/// Step 14.4 extracts this from the prior monolithic
/// [`build_timeline_argv`] so future filter types (transitions in
/// 14.5; volume / speed in Step 15; drawtext in Step 16) can compose
/// without rewriting the same builder. Behaviour with empty
/// `transitions` is identical to the pre-extract code.
///
/// The planner doesn't take ownership of the segments or care about
/// the input source — callers feed the slice by reference.
pub struct FilterPlanner<'a> {
    segments: &'a [TimelineSegment],
    transitions: &'a [TransitionPlan],
}

/// Output of [`FilterPlanner::plan`]. Carries everything the caller
/// needs to splice into an ffmpeg argv: the filter graph string and
/// the `[outv]` / `[outa]` map labels (the planner picks these so
/// 14.5's xfade chain can rename them if it needs intermediate
/// stages without breaking the caller's `-map` args).
#[derive(Debug, Clone)]
pub struct FilterPlan {
    /// Value for `-filter_complex`.
    pub filter_complex: String,
    /// Label for `-map` on the video output (typically `[outv]`).
    pub video_out_label: String,
    /// Label for `-map` on the audio output (typically `[outa]`).
    pub audio_out_label: String,
}

impl<'a> FilterPlanner<'a> {
    /// Construct a planner over segments + transitions.
    pub fn new(
        segments: &'a [TimelineSegment],
        transitions: &'a [TransitionPlan],
    ) -> Self {
        Self {
            segments,
            transitions,
        }
    }

    /// Build the filter complex + output labels.
    ///
    /// With no transitions: emits the same monolithic
    /// `[0:v:0][0:a:0]…concat=n=N:v=1:a=1[outv][outa]` graph the
    /// pre-extract code produced.
    ///
    /// With transitions (Step 14.5): groups consecutive segments
    /// connected by a transition into "chunks." Each pair-chunk
    /// emits `xfade` + `acrossfade` filters into a `[xv<i>][xa<i>]`
    /// pair which then participates in the final concat in place
    /// of the two raw segment streams. Lone segments still feed in
    /// directly via `[i:v:0][i:a:0]`.
    ///
    /// v1 chunk policy: each segment can participate in at most one
    /// transition. If two transitions try to share the same middle
    /// segment (a chain of three), the second transition is dropped
    /// with a debug-trace log — the render still produces a valid
    /// output, just without that overlap. Multi-transition chains
    /// land in a future commit.
    pub fn plan(&self) -> FilterPlan {
        if self.transitions.is_empty() {
            return self.plan_no_transitions();
        }
        self.plan_with_transitions()
    }

    fn plan_no_transitions(&self) -> FilterPlan {
        let n = self.segments.len();
        let mut filter = String::new();
        // Pre-stage so per-segment effects (volume in 15.3, speed in
        // 15.4) prepend their filter chain before the concat. Each
        // call returns the (video, audio) labels to feed into concat.
        let inputs: Vec<(String, String)> = (0..n)
            .map(|i| stage_segment_inputs(&mut filter, i, &self.segments[i]))
            .collect();
        for (v, a) in &inputs {
            filter.push_str(v);
            filter.push_str(a);
        }
        filter.push_str(&format!("concat=n={n}:v=1:a=1[outv][outa]"));
        FilterPlan {
            filter_complex: filter,
            video_out_label: "[outv]".into(),
            audio_out_label: "[outa]".into(),
        }
    }

    fn plan_with_transitions(&self) -> FilterPlan {
        let n = self.segments.len();

        // Build a "next-segment-of-the-same-chunk" map. seg `i`'s
        // partner is `j` iff there's a transition between them. We
        // enforce v1 single-transition-per-segment by only
        // remembering the *first* transition for any given segment.
        let mut paired_with: Vec<Option<&TransitionPlan>> = vec![None; n];
        for t in self.transitions {
            if t.from_segment_index >= n || t.to_segment_index != t.from_segment_index + 1 {
                tracing::debug!(
                    transition = ?t,
                    "FilterPlanner: dropping transition with non-adjacent indices"
                );
                continue;
            }
            // Either segment already taken? Drop this one.
            if paired_with[t.from_segment_index].is_some()
                || paired_with[t.to_segment_index].is_some()
            {
                tracing::debug!(
                    transition = ?t,
                    "FilterPlanner: dropping transition (segment already part of a chunk)",
                );
                continue;
            }
            paired_with[t.from_segment_index] = Some(t);
        }

        let mut filter = String::new();

        // Pre-stage each segment's video / audio inputs. Step 15.3
        // adds the per-segment volume filter here: when a segment
        // carries an awidat.volume effect, its audio stream goes
        // through `volume=<v>` first, producing a [av<i>] label that
        // downstream graph nodes use in place of [i:a:0]. Speed
        // lands in 15.4 with a parallel pass on video + atempo on
        // audio.
        let inputs: Vec<(String, String)> = (0..n)
            .map(|i| stage_segment_inputs(&mut filter, i, &self.segments[i]))
            .collect();

        // Track the order of concat input pairs (each entry is a
        // pre-built (video_label, audio_label) ready to drop in).
        let mut concat_inputs: Vec<(String, String)> = Vec::with_capacity(n);

        let mut i = 0;
        let mut chunk_id: usize = 0;
        while i < n {
            if let Some(t) = paired_with[i] {
                let j = t.to_segment_index;
                let v_label = format!("[xv{chunk_id}]");
                let a_label = format!("[xa{chunk_id}]");
                let xfade_kind = map_transition_kind(&t.kind);
                // xfade offset = the from-segment's duration minus
                // the transition duration. Both inputs must be at
                // the cut point at offset `from.duration - duration`.
                let from_dur = self.segments[i].duration_s;
                let offset = (from_dur - t.duration_s).max(0.0);
                filter.push_str(&format!(
                    "{from_v}{to_v}xfade=transition={kind}:duration={dur}:offset={off}{out};",
                    from_v = inputs[i].0,
                    to_v = inputs[j].0,
                    kind = xfade_kind,
                    dur = t.duration_s,
                    off = offset,
                    out = v_label,
                ));
                filter.push_str(&format!(
                    "{from_a}{to_a}acrossfade=d={dur}{out};",
                    from_a = inputs[i].1,
                    to_a = inputs[j].1,
                    dur = t.duration_s,
                    out = a_label,
                ));
                concat_inputs.push((v_label, a_label));
                chunk_id += 1;
                i += 2;
            } else {
                concat_inputs.push((inputs[i].0.clone(), inputs[i].1.clone()));
                i += 1;
            }
        }

        // Tail: single-input concat would just rename, so when we
        // have one chunk it might be a paired-xfade output already.
        // ffmpeg's concat takes n>=1; we always wrap so the caller's
        // `-map [outv] -map [outa]` is uniform.
        for (v, a) in &concat_inputs {
            filter.push_str(v);
            filter.push_str(a);
        }
        filter.push_str(&format!(
            "concat=n={n}:v=1:a=1[outv][outa]",
            n = concat_inputs.len(),
        ));

        FilterPlan {
            filter_complex: filter,
            video_out_label: "[outv]".into(),
            audio_out_label: "[outa]".into(),
        }
    }
}

/// Build the per-segment video / audio entry labels into `filter`.
/// When the segment has no awidat.volume effect (or it's exactly
/// 1.0), the labels are the raw `[i:v:0]` / `[i:a:0]` from ffmpeg's
/// input streams. When awidat.volume is set to a non-unity value, a
/// `[i:a:0]volume=<v>[av<i>]` filter chain prepends and the audio
/// label becomes `[av<i>]`.
///
/// Step 15.4 will extend this to also handle awidat.speed via
/// `setpts` on video + chained `atempo` on audio.
fn stage_segment_inputs(
    filter: &mut String,
    i: usize,
    seg: &TimelineSegment,
) -> (String, String) {
    let video_label = format!("[{i}:v:0]");
    let mut audio_label = format!("[{i}:a:0]");
    if let Some(v) = seg.volume
        && (v - 1.0).abs() > 1e-9
    {
        let av = format!("[av{i}]");
        filter.push_str(&format!("[{i}:a:0]volume={v}{av};"));
        audio_label = av;
    }
    (video_label, audio_label)
}

/// Map an OTIO transition kind to an ffmpeg `xfade=transition=` name.
/// Unknown kinds pass through verbatim — ffmpeg will reject them at
/// render time with a clear error, which is better than silently
/// substituting a wrong-but-valid kind.
fn map_transition_kind(kind: &str) -> String {
    match kind {
        "SMPTE_Dissolve" => "fade".into(),
        "awidat.fade_in" => "fadeblack".into(),
        "awidat.fade_out" => "fadeblack".into(),
        other => other.to_string(),
    }
}

/// Build the ffmpeg argv that concats `segs` into `output_path` with
/// a single re-encode. The re-encode kills the DTS-seam scratch that
/// stream-copy concat produces at non-keyframe-aligned cut points.
/// libx264 medium preset / CRF 20, AAC 192k — universal compatibility.
///
/// Internally delegates the filter-graph construction to
/// [`FilterPlanner`] (Step 14.4 extraction); behaviour is byte-
/// identical to the prior monolithic builder. Callers wanting
/// transitions should use [`build_timeline_argv_with_transitions`]
/// (Step 14.5).
pub fn build_timeline_argv(segs: &[TimelineSegment], output_path: &Path) -> Vec<String> {
    build_timeline_argv_with_transitions(segs, &[], output_path)
}

/// Like [`build_timeline_argv`] but accepts a transitions slice that
/// gets composed into the filter graph. Step 14.4 ships the
/// pass-through path (transitions slice is asserted empty by the
/// FilterPlanner); 14.5 wires the xfade path when transitions are
/// non-empty.
pub fn build_timeline_argv_with_transitions(
    segs: &[TimelineSegment],
    transitions: &[TransitionPlan],
    output_path: &Path,
) -> Vec<String> {
    let mut argv = vec!["-y".to_string(), "-loglevel".into(), "info".into()];
    for s in segs {
        argv.extend([
            "-ss".into(),
            format!("{}", s.start_s),
            "-t".into(),
            format!("{}", s.duration_s),
            "-i".into(),
            s.asset_path.to_string_lossy().into_owned(),
        ]);
    }
    let plan = FilterPlanner::new(segs, transitions).plan();
    argv.extend([
        "-filter_complex".into(),
        plan.filter_complex,
        "-map".into(),
        plan.video_out_label,
        "-map".into(),
        plan.audio_out_label,
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "20".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        output_path.to_string_lossy().into_owned(),
    ]);
    argv
}

/// One-call helper: walk OTIO, build the spec, return it. The output
/// path is `<project_root>/renders/timeline-<HHMMSS>.mp4` — same
/// naming `start_render scope=timeline` uses, so the agent and the
/// desktop produce indistinguishable artifacts.
pub fn build_timeline_render_spec(
    project_root: &Path,
) -> Result<RenderJobSpec, RenderTimelineError> {
    let (segs, transitions) = collect_timeline_plan(project_root)?;
    if segs.is_empty() {
        return Err(RenderTimelineError::EmptyTimeline);
    }
    // Total duration accounts for each transition's overlap — the
    // visual timeline is shorter than the sum of segment durations
    // by the cumulative transition durations.
    let raw_total: f64 = segs.iter().map(|s| s.duration_s).sum();
    let trans_total: f64 = transitions.iter().map(|t| t.duration_s).sum();
    let total_duration_s = (raw_total - trans_total).max(0.0);
    let renders_dir = project_root.join("renders");
    let timestamp = Utc::now().format("%H%M%S");
    let output_path = renders_dir.join(format!("timeline-{}.mp4", timestamp));
    let argv = build_timeline_argv_with_transitions(&segs, &transitions, &output_path);
    Ok(RenderJobSpec {
        args: argv,
        total_duration_s: Some(total_duration_s),
        cwd: Some(project_root.to_path_buf()),
        output_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild,
        TimeRange as OtioRange, Timeline, Track, TrackChild, TrackKind,
    };
    use std::fs;

    fn write_fixture_project(dir: &Path) -> PathBuf {
        let asset_rel = "raw/x.mp4";
        fs::create_dir_all(dir.join("raw")).unwrap();
        fs::write(dir.join(asset_rel), b"stub").unwrap();
        let mut clip = Clip::empty("c1".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new(asset_rel));
        clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(2.0 * 24.0, 24.0),
        ));
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        let mut tl = Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        tl.tracks = stack;
        let otio_path = dir.join(files::OTIO);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    #[test]
    fn no_otio_returns_no_otio_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = build_timeline_render_spec(dir.path()).unwrap_err();
        assert!(matches!(err, RenderTimelineError::NoOtio(_)));
    }

    #[test]
    fn empty_otio_returns_empty_timeline() {
        let dir = tempfile::tempdir().unwrap();
        // Init an OTIO file with no tracks.
        awidat_proto::project::Project::init(dir.path()).unwrap();
        let err = build_timeline_render_spec(dir.path()).unwrap_err();
        assert!(matches!(err, RenderTimelineError::EmptyTimeline));
    }

    #[test]
    fn fixture_project_produces_concat_argv() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project(dir.path());
        let spec = build_timeline_render_spec(dir.path()).unwrap();
        assert!(spec.total_duration_s.unwrap() > 1.9);
        // Concat filter present, libx264 (re-encode, not stream-copy).
        let cmd = spec.args.join(" ");
        assert!(cmd.contains("concat=n=1:v=1:a=1"));
        assert!(cmd.contains("libx264"));
        assert!(!cmd.contains(" copy "));
        // Output under renders/ with timeline-<HHMMSS>.mp4 naming.
        assert!(spec.output_path.starts_with(dir.path().join("renders")));
        assert!(spec
            .output_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("timeline-"));
    }

    #[test]
    fn missing_asset_returns_missing_asset_error() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project(dir.path());
        // Delete the asset; spec build should fail with MissingAsset.
        fs::remove_file(dir.path().join("raw/x.mp4")).unwrap();
        let err = build_timeline_render_spec(dir.path()).unwrap_err();
        assert!(matches!(err, RenderTimelineError::MissingAsset { .. }));
    }

    /// Build a basic TimelineSegment for tests — no volume / speed
    /// effects. Saves repeating `..Default::default()` 12 times.
    fn seg(path: &str, start: f64, dur: f64) -> TimelineSegment {
        TimelineSegment {
            asset_path: PathBuf::from(path),
            start_s: start,
            duration_s: dur,
            ..Default::default()
        }
    }

    #[test]
    fn filter_planner_with_no_transitions_emits_legacy_concat_graph() {
        // Step 14.4 extracted FilterPlanner from build_timeline_argv;
        // this test pins the no-transition graph shape so future
        // commits can't drift it without noticing.
        let segs = vec![
            seg("/tmp/a.mp4", 0.0, 2.0),
            seg("/tmp/b.mp4", 1.0, 3.0),
        ];
        let plan = FilterPlanner::new(&segs, &[]).plan();
        assert_eq!(
            plan.filter_complex,
            "[0:v:0][0:a:0][1:v:0][1:a:0]concat=n=2:v=1:a=1[outv][outa]",
        );
        assert_eq!(plan.video_out_label, "[outv]");
        assert_eq!(plan.audio_out_label, "[outa]");
    }

    #[test]
    fn filter_planner_with_one_transition_emits_xfade_pair() {
        let segs = vec![seg("/tmp/a.mp4", 0.0, 5.0), seg("/tmp/b.mp4", 0.0, 4.0)];
        let trans = vec![TransitionPlan {
            from_segment_index: 0,
            to_segment_index: 1,
            kind: "SMPTE_Dissolve".into(),
            duration_s: 1.0,
        }];
        let plan = FilterPlanner::new(&segs, &trans).plan();
        // xfade with kind=fade (mapped from SMPTE_Dissolve), offset =
        // from.duration - transition.duration = 4.0.
        assert!(
            plan.filter_complex
                .contains("xfade=transition=fade:duration=1:offset=4"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // acrossfade for audio.
        assert!(
            plan.filter_complex.contains("acrossfade=d=1"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // The chunk pair feeds into a 1-input concat (the merged xfade
        // counts as one input pair).
        assert!(
            plan.filter_complex.contains("concat=n=1:v=1:a=1[outv][outa]"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_with_transition_in_middle_of_three_segments() {
        // [A, B, C] with a transition between A and B → concat n=2:
        // input 1 = xfade(A, B), input 2 = C alone.
        let segs = vec![
            seg("/tmp/a.mp4", 0.0, 3.0),
            seg("/tmp/b.mp4", 0.0, 4.0),
            seg("/tmp/c.mp4", 0.0, 2.0),
        ];
        let trans = vec![TransitionPlan {
            from_segment_index: 0,
            to_segment_index: 1,
            kind: "SMPTE_Dissolve".into(),
            duration_s: 0.5,
        }];
        let plan = FilterPlanner::new(&segs, &trans).plan();
        assert!(plan.filter_complex.contains("xfade="));
        // Concat takes 2 inputs: chunk(A,B) + raw C.
        assert!(plan.filter_complex.contains("concat=n=2:v=1:a=1[outv][outa]"));
        // C's raw streams must appear in the concat input list.
        assert!(plan.filter_complex.contains("[2:v:0][2:a:0]"));
    }

    #[test]
    fn filter_planner_drops_chained_transitions() {
        // [A, B, C] with transitions A-B AND B-C: B can only belong
        // to one chunk in v1. The first transition wins; the second
        // is dropped (with a debug-trace log we can't easily assert
        // here). Resulting concat: xfade(A,B) + raw C.
        let segs = vec![
            seg("/tmp/a.mp4", 0.0, 3.0),
            seg("/tmp/b.mp4", 0.0, 4.0),
            seg("/tmp/c.mp4", 0.0, 2.0),
        ];
        let trans = vec![
            TransitionPlan {
                from_segment_index: 0,
                to_segment_index: 1,
                kind: "SMPTE_Dissolve".into(),
                duration_s: 0.5,
            },
            TransitionPlan {
                from_segment_index: 1,
                to_segment_index: 2,
                kind: "SMPTE_Dissolve".into(),
                duration_s: 0.5,
            },
        ];
        let plan = FilterPlanner::new(&segs, &trans).plan();
        // Exactly one xfade (A-B).
        let xfade_count = plan.filter_complex.matches("xfade=").count();
        assert_eq!(xfade_count, 1, "filter graph: {}", plan.filter_complex);
        // C still in the concat as a raw input.
        assert!(plan.filter_complex.contains("[2:v:0][2:a:0]"));
    }

    #[test]
    fn filter_planner_emits_volume_filter_when_segment_carries_value() {
        // Step 15.3: a segment with volume=0.5 prepends a volume=
        // filter that produces [av0], then the concat consumes
        // [av0] in place of the raw [0:a:0].
        let mut s0 = seg("/tmp/a.mp4", 0.0, 2.0);
        s0.volume = Some(0.5);
        let s1 = seg("/tmp/b.mp4", 0.0, 3.0);
        let plan = FilterPlanner::new(&[s0, s1], &[]).plan();
        assert!(
            plan.filter_complex.contains("[0:a:0]volume=0.5[av0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // Concat input pair for seg 0 uses [av0] for audio, raw for video.
        assert!(
            plan.filter_complex.contains("[0:v:0][av0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // Seg 1 has no volume effect — raw labels.
        assert!(
            plan.filter_complex.contains("[1:v:0][1:a:0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_skips_volume_filter_at_unity() {
        // volume = 1.0 is the no-op default; no filter should land.
        let mut s0 = seg("/tmp/a.mp4", 0.0, 2.0);
        s0.volume = Some(1.0);
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            !plan.filter_complex.contains("volume="),
            "filter graph should skip unity volume: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_volume_threads_through_xfade_pair() {
        // Volume on the to-segment of an xfade pair must feed the
        // [av<i>] label into acrossfade, not the raw [i:a:0].
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let mut s1 = seg("/tmp/b.mp4", 0.0, 4.0);
        s1.volume = Some(0.3);
        let trans = vec![TransitionPlan {
            from_segment_index: 0,
            to_segment_index: 1,
            kind: "SMPTE_Dissolve".into(),
            duration_s: 1.0,
        }];
        let plan = FilterPlanner::new(&[s0, s1], &trans).plan();
        assert!(
            plan.filter_complex.contains("[1:a:0]volume=0.3[av1]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // acrossfade reads [0:a:0] (no volume on s0) and [av1] (volume on s1).
        assert!(
            plan.filter_complex.contains("[0:a:0][av1]acrossfade"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn build_timeline_argv_unchanged_after_extraction() {
        // Behaviour-preservation guard for 14.4. The argv produced
        // for a multi-segment fixture must be exactly what the old
        // monolithic builder produced. If 14.5 changes the
        // no-transitions graph, this test is the canary.
        let segs = vec![seg("/tmp/a.mp4", 0.0, 2.0), seg("/tmp/b.mp4", 1.0, 3.0)];
        let argv = build_timeline_argv(&segs, Path::new("/tmp/out.mp4"));
        let cmd = argv.join(" ");
        // Two -ss / -t / -i triples preceded by `-y -loglevel info`.
        assert!(cmd.starts_with("-y -loglevel info -ss 0 -t 2 -i /tmp/a.mp4 -ss 1 -t 3 -i /tmp/b.mp4"));
        assert!(cmd.contains(
            "-filter_complex [0:v:0][0:a:0][1:v:0][1:a:0]concat=n=2:v=1:a=1[outv][outa] \
             -map [outv] -map [outa]",
        ));
        assert!(cmd.ends_with("/tmp/out.mp4"));
    }
}
