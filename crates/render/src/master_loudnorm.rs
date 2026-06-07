//! Two-pass master-bus `loudnorm` orchestration.
//!
//! EBU-compliant loudness leveling needs FFmpeg's two-pass workflow:
//!
//! 1. **Measure pass** — run `loudnorm=I=<lufs>:LRA=<lra>:TP=<tp>:print_format=json`
//!    over a null sink (`-f null -`). The filter prints a JSON block to
//!    stderr with the measured integrated loudness, true peak, LRA,
//!    noise floor threshold, and target offset.
//! 2. **Apply pass** — run the normal render with `measured_I=`,
//!    `measured_LRA=`, `measured_TP=`, `measured_thresh=`, `offset=`,
//!    `linear=true`, and `print_format=summary` substituted into the
//!    master audio filter chain so ffmpeg can do a single, well-targeted
//!    linear normalization.
//!
//! The single-pass mode (via [`crate::timeline::LoudnessTargetPlan`]) is
//! the default; this module is purely additive and opt-in via
//! `metadata.montage.extra["master_loudnorm"]` on the timeline.

use std::path::{Path, PathBuf};

use montage_proto::montage_meta::MontageTimelineMetadata;
use montage_proto::otio::Timeline as OtioTimeline;
use montage_proto::project::files;
use serde::Deserialize;
use thiserror::Error;

use crate::job::{JobManager, JobState, JobStatus, RenderJobSpec};
use crate::timeline::{RenderTimelineError, build_timeline_render_spec};

/// Render-side projection of the proto `MasterLoudnorm` settings.
///
/// Mirrors [`montage_proto::professional::MasterLoudnorm`] but stays in
/// the render crate so callers don't need to import proto types just to
/// build a two-pass spec.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct MasterLoudnormPlan {
    /// Integrated loudness target in LUFS (negative).
    pub target_lufs: f64,
    /// Loudness range target in LU.
    pub target_lra: f64,
    /// True peak ceiling in dBTP (negative or zero).
    pub target_tp: f64,
}

impl MasterLoudnormPlan {
    /// True iff the plan's targets are usable as ffmpeg `loudnorm` args.
    pub fn is_valid(&self) -> bool {
        self.target_lufs.is_finite()
            && self.target_lufs < 0.0
            && self.target_lra.is_finite()
            && self.target_lra > 0.0
            && self.target_tp.is_finite()
            && self.target_tp <= 0.0
    }
}

/// Loudness measurements parsed out of ffmpeg's stderr after the
/// measure pass. Each field corresponds to a key in the JSON block
/// emitted by `loudnorm=...:print_format=json`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasuredLoudnorm {
    /// `input_i` — measured integrated loudness in LUFS.
    pub measured_i: f64,
    /// `input_lra` — measured loudness range in LU.
    pub measured_lra: f64,
    /// `input_tp` — measured true peak in dBTP.
    pub measured_tp: f64,
    /// `input_thresh` — measured relative threshold for the integrated
    /// loudness gate.
    pub measured_thresh: f64,
    /// `target_offset` — gain offset ffmpeg recommends applying to hit
    /// the target loudness on the second pass.
    pub target_offset: f64,
}

/// Errors from the two-pass orchestrator.
#[derive(Debug, Error)]
pub enum MasterLoudnormError {
    /// Project root has no `project.otio.json`.
    #[error("no project.otio.json at {0}")]
    NoOtio(std::path::PathBuf),
    /// Timeline parse failed.
    #[error("timeline parse failed: {0}")]
    OtioParse(String),
    /// Timeline metadata is missing the `master_loudnorm` settings.
    #[error("timeline has no master_loudnorm settings on metadata.montage.extra")]
    MissingSettings,
    /// `master_loudnorm` settings are present but malformed or out-of-range.
    #[error("master_loudnorm settings are invalid: {0}")]
    InvalidSettings(String),
    /// Underlying timeline render-spec build failed.
    #[error(transparent)]
    Timeline(#[from] RenderTimelineError),
    /// Could not locate the master loudnorm JSON block in stderr.
    #[error("loudnorm measurement JSON not found in ffmpeg stderr")]
    MeasureJsonNotFound,
    /// Loudnorm JSON parsed but a required field was missing or malformed.
    #[error("loudnorm measurement JSON is incomplete: {0}")]
    MeasureJsonIncomplete(String),
    /// A render-job runner submission failed.
    #[error("render job runner failed: {0}")]
    Runner(#[from] RenderJobRunnerError),
}

/// Read the `master_loudnorm` block off the project's OTIO metadata.
/// Returns `Ok(None)` when the timeline does not opt in to two-pass.
pub fn read_master_loudnorm_plan(
    project_root: &Path,
) -> Result<Option<MasterLoudnormPlan>, MasterLoudnormError> {
    let otio_path = project_root.join(files::OTIO);
    if !otio_path.exists() {
        return Err(MasterLoudnormError::NoOtio(otio_path));
    }
    let raw = std::fs::read_to_string(&otio_path)
        .map_err(|e| MasterLoudnormError::OtioParse(e.to_string()))?;
    let tl: OtioTimeline =
        serde_json::from_str(&raw).map_err(|e| MasterLoudnormError::OtioParse(e.to_string()))?;
    Ok(tl
        .metadata
        .montage
        .as_ref()
        .and_then(read_master_loudnorm_from_meta))
}

/// Extract a [`MasterLoudnormPlan`] off the timeline's
/// `metadata.montage.extra["master_loudnorm"]` JSON value.
pub fn read_master_loudnorm_from_meta(
    metadata: &MontageTimelineMetadata,
) -> Option<MasterLoudnormPlan> {
    let value = metadata.extra.get("master_loudnorm")?;
    serde_json::from_value::<MasterLoudnormPlan>(value.clone())
        .ok()
        .filter(MasterLoudnormPlan::is_valid)
}

/// Parse a loudnorm measurement JSON block out of ffmpeg's stderr tail.
///
/// `loudnorm=...:print_format=json` appends a JSON object to stderr; the
/// surrounding lines are normal ffmpeg log noise. This finds the last
/// `{ ... }` block in the stream and pulls the fields we need.
pub fn parse_loudnorm_measure_json(stderr: &str) -> Result<MeasuredLoudnorm, MasterLoudnormError> {
    let (start, end) = locate_json_block(stderr).ok_or(MasterLoudnormError::MeasureJsonNotFound)?;
    let raw = &stderr[start..=end];
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| MasterLoudnormError::MeasureJsonIncomplete(e.to_string()))?;
    let pick = |key: &str| -> Result<f64, MasterLoudnormError> {
        let v = value
            .get(key)
            .ok_or_else(|| MasterLoudnormError::MeasureJsonIncomplete(format!("missing {key}")))?;
        match v {
            serde_json::Value::Number(n) => n.as_f64().ok_or_else(|| {
                MasterLoudnormError::MeasureJsonIncomplete(format!("{key} not f64"))
            }),
            serde_json::Value::String(s) => s
                .trim()
                .parse::<f64>()
                .map_err(|e| MasterLoudnormError::MeasureJsonIncomplete(format!("{key}: {e}"))),
            other => Err(MasterLoudnormError::MeasureJsonIncomplete(format!(
                "{key} unexpected JSON type: {other:?}"
            ))),
        }
    };
    Ok(MeasuredLoudnorm {
        measured_i: pick("input_i")?,
        measured_lra: pick("input_lra")?,
        measured_tp: pick("input_tp")?,
        measured_thresh: pick("input_thresh")?,
        target_offset: pick("target_offset")?,
    })
}

/// Find the last balanced `{ ... }` block in `s`. Used to extract
/// loudnorm's trailing JSON block out of free-form stderr.
fn locate_json_block(s: &str) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    let last_close = bytes.iter().rposition(|&b| b == b'}')?;
    let mut depth = 1usize;
    let mut i = last_close;
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b'}' => depth += 1,
            b'{' => {
                depth -= 1;
                if depth == 0 {
                    return Some((i, last_close));
                }
            }
            _ => {}
        }
    }
    None
}

/// Build the **pass 1 (measure)** render spec for the project's master
/// loudnorm settings.
///
/// The returned spec runs against `-f null -` so ffmpeg produces no
/// output file; its stderr will contain the loudnorm JSON block that
/// [`parse_loudnorm_measure_json`] consumes.
pub fn build_master_loudnorm_measure_spec(
    project_root: &Path,
) -> Result<RenderJobSpec, MasterLoudnormError> {
    let settings =
        read_master_loudnorm_plan(project_root)?.ok_or(MasterLoudnormError::MissingSettings)?;
    let mut base = build_timeline_render_spec(project_root)?;
    let target_filter = loudnorm_filter_measure(&settings);
    inject_master_loudnorm_filter(&mut base.args, &target_filter);
    retarget_argv_to_null_muxer(&mut base);
    Ok(base)
}

/// Build the **pass 2 (apply)** render spec, plugging the measured
/// values from pass 1 into the master loudnorm filter.
pub fn build_master_loudnorm_apply_spec(
    project_root: &Path,
    measured: MeasuredLoudnorm,
) -> Result<RenderJobSpec, MasterLoudnormError> {
    let settings =
        read_master_loudnorm_plan(project_root)?.ok_or(MasterLoudnormError::MissingSettings)?;
    let mut base = build_timeline_render_spec(project_root)?;
    let target_filter = loudnorm_filter_apply(&settings, &measured);
    inject_master_loudnorm_filter(&mut base.args, &target_filter);
    Ok(base)
}

/// Emit the master loudnorm filter args for pass 1 (measure).
fn loudnorm_filter_measure(settings: &MasterLoudnormPlan) -> String {
    format!(
        "loudnorm=I={}:LRA={}:TP={}:print_format=json",
        fmt(settings.target_lufs),
        fmt(settings.target_lra),
        fmt(settings.target_tp),
    )
}

/// Emit the master loudnorm filter args for pass 2 (apply), with the
/// measured values from pass 1 inlined.
fn loudnorm_filter_apply(settings: &MasterLoudnormPlan, measured: &MeasuredLoudnorm) -> String {
    format!(
        "loudnorm=I={i}:LRA={lra}:TP={tp}:measured_I={mi}:measured_LRA={mlra}:measured_TP={mtp}:measured_thresh={mthresh}:offset={offset}:linear=true:print_format=summary",
        i = fmt(settings.target_lufs),
        lra = fmt(settings.target_lra),
        tp = fmt(settings.target_tp),
        mi = fmt(measured.measured_i),
        mlra = fmt(measured.measured_lra),
        mtp = fmt(measured.measured_tp),
        mthresh = fmt(measured.measured_thresh),
        offset = fmt(measured.target_offset),
    )
}

/// Format a number for embedding in an ffmpeg filter expression. Trims
/// trailing zeros and normalizes `-0` to `0` so command-strings stay
/// stable across machines.
fn fmt(value: f64) -> String {
    let mut s = format!("{value:.6}");
    while s.contains('.') && s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    if s == "-0" { "0".into() } else { s }
}

/// Rewrite `argv` so the master audio chain ends in our custom loudnorm
/// filter, replacing whatever single-pass `loudnorm=...` chunk the
/// timeline already emitted (if any) and rewriting the `-map` for audio
/// to the new label.
fn inject_master_loudnorm_filter(argv: &mut [String], loudnorm_filter: &str) {
    let Some(fc_idx) = filter_complex_value_index(argv) else {
        // No filter graph — nothing to splice into. Caller must have
        // built a non-trivial spec, but bail gracefully.
        return;
    };
    let graph = argv[fc_idx].clone();
    let (audio_in_label, audio_out_label) = current_audio_labels(&graph);

    // Strip the existing trailing master loudnorm chunk, if the
    // timeline emitted one via `append_timeline_loudness_filter`.
    let mut new_graph = strip_existing_master_loudnorm(&graph);

    if !new_graph.is_empty() && !new_graph.ends_with(';') {
        new_graph.push(';');
    }
    new_graph.push_str(&format!(
        "{audio_in_label}aresample=async=1:first_pts=0,{loudnorm_filter}{MASTER_LOUDNORM_LABEL}",
    ));

    argv[fc_idx] = new_graph;

    // Repoint the audio `-map` arg at our new master label.
    if let Some(map_idx) = audio_map_index(argv, &audio_out_label) {
        argv[map_idx] = MASTER_LOUDNORM_LABEL.to_string();
    }
}

const MASTER_LOUDNORM_LABEL: &str = "[mastera]";

/// Locate the value index of `-filter_complex` in `argv` (i.e. one past
/// the flag).
fn filter_complex_value_index(argv: &[String]) -> Option<usize> {
    argv.iter()
        .position(|s| s == "-filter_complex")
        .and_then(|i| argv.get(i + 1).map(|_| i + 1))
}

/// Identify the current audio-in and audio-out labels of the graph so
/// we can splice a new master chain onto the end.
fn current_audio_labels(graph: &str) -> (String, String) {
    // The timeline always closes with either `[outa]` (no loudness
    // target) or `[mastera]` (single-pass loudness target). Both are
    // valid splice points; we read whichever appears last.
    for candidate in [MASTER_LOUDNORM_LABEL, "[outa]"] {
        if graph.contains(candidate) {
            return (candidate.to_string(), candidate.to_string());
        }
    }
    // Conservative fallback: chain off `[outa]` even if not literally
    // present so the resulting graph fails loudly at ffmpeg parse time.
    ("[outa]".to_string(), "[outa]".to_string())
}

/// Remove the trailing `;[label]aresample=async=1:first_pts=0,loudnorm=...
/// [mastera]` chunk emitted by the single-pass timeline path, if any.
/// Leaves single-pass-free graphs untouched.
fn strip_existing_master_loudnorm(graph: &str) -> String {
    let Some(label_pos) = graph.rfind(MASTER_LOUDNORM_LABEL) else {
        return graph.to_string();
    };
    // Walk backwards from the [mastera] label to find the `;` that
    // separates this chunk from the rest of the graph. If we don't
    // find one, the entire trailing chunk is ours to drop.
    let prefix = &graph[..label_pos];
    if let Some(sep) = prefix.rfind(';') {
        graph[..sep].to_string()
    } else {
        String::new()
    }
}

/// Locate the argv index of the audio `-map` value matching `label`.
fn audio_map_index(argv: &[String], label: &str) -> Option<usize> {
    for (i, w) in argv.iter().enumerate() {
        if w == "-map"
            && let Some(next) = argv.get(i + 1)
            && next == label
        {
            return Some(i + 1);
        }
    }
    None
}

/// Retarget the trailing output args of a render spec to a null muxer
/// so ffmpeg measures but writes no media. Used for pass 1.
fn retarget_argv_to_null_muxer(spec: &mut RenderJobSpec) {
    // Find the first occurrence of `-c:v` (start of the encode trailer
    // emitted by the timeline argv builders) and replace everything
    // from there to the end with `-f null -`. If the trailer is shaped
    // differently, fall back to appending `-f null -` after the maps.
    let trailer_start = spec
        .args
        .iter()
        .position(|s| s == "-c:v")
        .unwrap_or(spec.args.len());
    spec.args.truncate(trailer_start);
    spec.args.extend(["-f".into(), "null".into(), "-".into()]);
    // Pass 1 writes to a null sink, so the original output path is no
    // longer meaningful — but callers can still observe it for
    // diagnostics so we leave `spec.output_path` alone.
}

/// Outcome of a single render-job submission as seen by the master
/// loudnorm orchestrator. Mirrors only the fields the orchestrator needs
/// out of [`crate::job::JobStatus`] so the trait stays mockable without
/// depending on the full status surface.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderRunnerOutput {
    /// Captured ffmpeg stderr (tail-truncated). Pass 1's stderr carries
    /// the loudnorm JSON block; pass 2's is kept for diagnostics.
    pub stderr: String,
    /// Path the runner wrote to. For pass 1 this is the spec's nominal
    /// output (no media is actually written — ffmpeg used `-f null -`);
    /// for pass 2 this is the real encoded artifact.
    pub output_path: PathBuf,
    /// ffmpeg exit code, if the process exited.
    pub exit_code: Option<i32>,
}

/// Errors a [`RenderJobRunner`] can report to the orchestrator.
#[derive(Debug, Error)]
pub enum RenderJobRunnerError {
    /// The runner could not submit or complete the job. The message is
    /// implementation-defined (e.g. ffmpeg spawn failure, non-zero exit).
    #[error("render job failed: {0}")]
    Failed(String),
}

/// Thin abstraction over `JobManager` so the master loudnorm
/// orchestrator can run end-to-end against a mock in tests without
/// touching ffmpeg. The single method submits one render spec and
/// blocks until terminal, returning the captured stderr + output path.
///
/// Implementations should be `Send + Sync` if used across threads; for
/// the orchestrator's purposes a single-threaded call is sufficient.
pub trait RenderJobRunner {
    /// Submit `spec`, wait for it to reach a terminal state, and return
    /// the captured stderr + output path. Implementations decide how
    /// long to wait (e.g. via a polling loop) and translate non-zero
    /// exits into [`RenderJobRunnerError::Failed`].
    fn run(&self, spec: &RenderJobSpec) -> Result<RenderRunnerOutput, RenderJobRunnerError>;
}

/// Default polling cadence the JobManager-backed runner uses while
/// waiting for a job to reach a terminal state.
const JOB_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

/// Production [`RenderJobRunner`] backed by [`JobManager`]. Reuses the
/// existing async manager but exposes a synchronous `run` for the
/// orchestrator (which is itself sync — the orchestrator does not need
/// to interleave with other async work, only to thread two ffmpeg
/// invocations).
pub struct JobManagerRunner {
    manager: JobManager,
    runtime: tokio::runtime::Handle,
}

impl JobManagerRunner {
    /// Build a runner against an existing `JobManager` and the tokio
    /// runtime handle the manager was created on.
    pub fn new(manager: JobManager, runtime: tokio::runtime::Handle) -> Self {
        Self { manager, runtime }
    }
}

impl RenderJobRunner for JobManagerRunner {
    fn run(&self, spec: &RenderJobSpec) -> Result<RenderRunnerOutput, RenderJobRunnerError> {
        let manager = self.manager.clone();
        let spec = spec.clone();
        let status: JobStatus = self.runtime.block_on(async move {
            let id = manager
                .start(spec)
                .await
                .map_err(|e| RenderJobRunnerError::Failed(e.to_string()))?;
            loop {
                let status = manager
                    .status(&id)
                    .await
                    .map_err(|e| RenderJobRunnerError::Failed(e.to_string()))?;
                if matches!(
                    status.state,
                    JobState::Done | JobState::Failed | JobState::Cancelled
                ) {
                    return Ok::<JobStatus, RenderJobRunnerError>(status);
                }
                tokio::time::sleep(JOB_POLL_INTERVAL).await;
            }
        })?;

        if !matches!(status.state, JobState::Done) {
            return Err(RenderJobRunnerError::Failed(format!(
                "job ended in non-Done state {:?} (exit={:?}): {}",
                status.state, status.exit_code, status.log_excerpt
            )));
        }

        Ok(RenderRunnerOutput {
            stderr: status.log_excerpt,
            output_path: status.output_path,
            exit_code: status.exit_code,
        })
    }
}

/// Run the full two-pass master loudnorm workflow against `runner`:
///
/// 1. Build the **measure** spec for `project_root`.
/// 2. Submit it to `runner`; capture pass-1 stderr.
/// 3. Parse the loudnorm JSON block out of that stderr.
/// 4. Build the **apply** spec with the parsed measurements substituted.
/// 5. Submit pass 2 and return its output.
///
/// This is the orchestrator the user-facing `start_render` tool should
/// call when a timeline opts into two-pass master loudnorm; wiring that
/// up is a separate slice. For now this function ships standalone so it
/// can be tested against a mock runner.
pub fn run_master_loudnorm_render<R: RenderJobRunner + ?Sized>(
    project_root: &Path,
    runner: &R,
) -> Result<RenderRunnerOutput, MasterLoudnormError> {
    let measure_spec = build_master_loudnorm_measure_spec(project_root)?;
    let measure_out = runner.run(&measure_spec)?;
    let measured = parse_loudnorm_measure_json(&measure_out.stderr)?;
    let apply_spec = build_master_loudnorm_apply_spec(project_root, measured)?;
    Ok(runner.run(&apply_spec)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_json_block_finds_last_braced_object() {
        let s = "ffmpeg log noise { not json } more noise\n{\n \"input_i\": \"-19.7\"\n}\n";
        let (start, end) = locate_json_block(s).unwrap();
        let extracted = &s[start..=end];
        assert!(extracted.starts_with('{'));
        assert!(extracted.ends_with('}'));
        assert!(extracted.contains("input_i"));
    }

    #[test]
    fn measure_filter_serializes_target_args() {
        let plan = MasterLoudnormPlan {
            target_lufs: -23.0,
            target_lra: 7.0,
            target_tp: -1.0,
        };
        assert_eq!(
            loudnorm_filter_measure(&plan),
            "loudnorm=I=-23:LRA=7:TP=-1:print_format=json"
        );
    }

    #[test]
    fn apply_filter_inlines_measured_values() {
        let plan = MasterLoudnormPlan {
            target_lufs: -23.0,
            target_lra: 7.0,
            target_tp: -1.0,
        };
        let measured = MeasuredLoudnorm {
            measured_i: -19.74,
            measured_lra: 7.3,
            measured_tp: -3.1,
            measured_thresh: -29.92,
            target_offset: 0.4,
        };
        let f = loudnorm_filter_apply(&plan, &measured);
        assert!(f.contains("measured_I=-19.74"), "{f}");
        assert!(f.contains("measured_LRA=7.3"), "{f}");
        assert!(f.contains("measured_TP=-3.1"), "{f}");
        assert!(f.contains("measured_thresh=-29.92"), "{f}");
        assert!(f.contains("offset=0.4"), "{f}");
        assert!(f.contains("linear=true"), "{f}");
        assert!(f.contains("print_format=summary"), "{f}");
    }

    #[test]
    fn strip_existing_master_loudnorm_removes_trailing_chunk() {
        let g = "[0:a:0]volume=0.5[a0];[a0]aresample=async=1:first_pts=0,loudnorm=I=-16:TP=-1.5:LRA=11[mastera]";
        let stripped = strip_existing_master_loudnorm(g);
        assert_eq!(stripped, "[0:a:0]volume=0.5[a0]");
    }

    #[test]
    fn strip_existing_master_loudnorm_noop_when_absent() {
        let g = "[0:a:0]volume=0.5[outa]";
        assert_eq!(strip_existing_master_loudnorm(g), g);
    }

    #[test]
    fn inject_replaces_audio_map_label() {
        let mut argv = vec![
            "-y".into(),
            "-filter_complex".into(),
            "[0:v:0]null[outv];[0:a:0]anull[outa]".into(),
            "-map".into(),
            "[outv]".into(),
            "-map".into(),
            "[outa]".into(),
            "-c:v".into(),
            "libx264".into(),
        ];
        inject_master_loudnorm_filter(&mut argv, "loudnorm=I=-16:LRA=11:TP=-1.5:print_format=json");
        let cmd = argv.join(" ");
        assert!(cmd.contains("[outa]aresample=async=1:first_pts=0,loudnorm=I=-16:LRA=11:TP=-1.5:print_format=json[mastera]"), "{cmd}");
        // Audio map must point at the new master label.
        let audio_map = argv
            .windows(2)
            .filter(|w| w[0] == "-map")
            .map(|w| w[1].clone())
            .collect::<Vec<_>>();
        assert!(
            audio_map.contains(&"[mastera]".to_string()),
            "{audio_map:?}"
        );
        assert!(!audio_map.contains(&"[outa]".to_string()), "{audio_map:?}");
    }

    #[test]
    fn inject_replaces_existing_master_loudnorm_chunk() {
        let mut argv = vec![
            "-filter_complex".into(),
            "[0:a:0]anull[outa];[outa]aresample=async=1:first_pts=0,loudnorm=I=-16:TP=-1.5:LRA=11[mastera]".into(),
            "-map".into(),
            "[mastera]".into(),
            "-c:v".into(),
            "libx264".into(),
        ];
        inject_master_loudnorm_filter(
            &mut argv,
            "loudnorm=I=-23:LRA=7:TP=-1:measured_I=-19:measured_LRA=7:measured_TP=-3:measured_thresh=-29:offset=0.4:linear=true:print_format=summary",
        );
        let cmd = argv.join(" ");
        // The old single-pass chunk must be gone.
        assert!(!cmd.contains("loudnorm=I=-16:TP=-1.5:LRA=11"), "{cmd}");
        // The new measured-pass chunk must be present.
        assert!(cmd.contains("measured_I=-19"), "{cmd}");
        assert!(cmd.contains("linear=true"), "{cmd}");
    }

    #[test]
    fn retarget_argv_to_null_muxer_drops_encode_trailer() {
        let mut spec = RenderJobSpec {
            args: vec![
                "-filter_complex".into(),
                "graph".into(),
                "-map".into(),
                "[mastera]".into(),
                "-c:v".into(),
                "libx264".into(),
                "-preset".into(),
                "veryfast".into(),
                "/tmp/out.mp4".into(),
            ],
            total_duration_s: None,
            backend: crate::RenderBackendKind::TimelineFfmpegReencode,
            cwd: None,
            output_path: std::path::PathBuf::from("/tmp/out.mp4"),
            input_paths: Vec::new(),
            manifest_path: None,
            limitations: Vec::new(),
            metadata: Default::default(),
        };
        retarget_argv_to_null_muxer(&mut spec);
        assert!(!spec.args.iter().any(|a| a == "libx264"));
        assert_eq!(
            spec.args.iter().rev().take(3).rev().collect::<Vec<_>>(),
            vec!["-f", "null", "-"]
        );
    }
}
