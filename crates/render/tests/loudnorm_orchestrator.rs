//! Integration test for the two-pass master loudnorm orchestrator.
//!
//! Verifies that [`awidat_render::run_master_loudnorm_render`] threads the
//! measure → parse → apply workflow correctly without actually invoking
//! ffmpeg. A mock [`awidat_render::RenderJobRunner`] returns a canned
//! pass-1 stderr containing the loudnorm JSON; the orchestrator must:
//!
//! 1. Submit the measure spec first (with `print_format=json` and the null
//!    muxer).
//! 2. Parse the JSON block out of pass-1's `log_excerpt`.
//! 3. Submit a second (apply) spec whose argv embeds the measured values
//!    parsed in step 2.
//! 4. Return pass-2's output.

#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::sync::Mutex;

use awidat_proto::awidat_meta::AwidatTimelineMetadata;
use awidat_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange, Timeline,
    Track, TrackChild, TrackKind,
};
use awidat_proto::project::files;
use awidat_render::{
    RenderJobRunner, RenderJobRunnerError, RenderJobSpec, RenderRunnerOutput,
    run_master_loudnorm_render,
};

fn write_project_with_master_loudnorm(dir: &Path) {
    let asset = "raw/a.mp4";
    std::fs::create_dir_all(dir.join("raw")).unwrap();
    std::fs::write(dir.join(asset), b"stub").unwrap();

    let mut clip = Clip::empty("clip-a".to_string());
    clip.media_reference = MediaReference::External(ExternalReference::new(asset));
    clip.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(2.0 * 24.0, 24.0),
    ));

    let mut track = Track::empty("V1", TrackKind::Video);
    track.children.push(TrackChild::Clip(clip));

    let mut tl = Timeline::empty("p");
    let mut stack = Stack::empty("root");
    stack.children.push(StackChild::Track(track));
    tl.tracks = stack;

    let mut meta = AwidatTimelineMetadata::default();
    meta.extra.insert(
        "master_loudnorm".into(),
        serde_json::json!({
            "target_lufs": -16.0,
            "target_lra": 11.0,
            "target_tp": -1.5,
        }),
    );
    tl.metadata.awidat = Some(meta);

    std::fs::write(
        dir.join(files::OTIO),
        serde_json::to_string_pretty(&tl).unwrap(),
    )
    .unwrap();
}

/// Test double: records each spec submitted and returns canned outputs
/// from a pre-seeded queue.
struct MockRunner {
    submitted: Mutex<Vec<RenderJobSpec>>,
    responses: Mutex<Vec<Result<RenderRunnerOutput, RenderJobRunnerError>>>,
}

impl MockRunner {
    fn new(responses: Vec<Result<RenderRunnerOutput, RenderJobRunnerError>>) -> Self {
        Self {
            submitted: Mutex::new(Vec::new()),
            responses: Mutex::new(responses),
        }
    }

    fn submitted(&self) -> Vec<RenderJobSpec> {
        self.submitted.lock().unwrap().clone()
    }
}

impl RenderJobRunner for MockRunner {
    fn run(&self, spec: &RenderJobSpec) -> Result<RenderRunnerOutput, RenderJobRunnerError> {
        self.submitted.lock().unwrap().push(spec.clone());
        let mut q = self.responses.lock().unwrap();
        if q.is_empty() {
            panic!("MockRunner ran out of canned responses");
        }
        q.remove(0)
    }
}

/// A fixture pass-1 stderr containing the loudnorm JSON block ffmpeg
/// emits at the end of `loudnorm=...:print_format=json -f null -`.
const PASS1_STDERR: &str = r#"
ffmpeg version irrelevant
[Parsed_loudnorm_0 @ 0x600003a00000]
{
    "input_i" : "-19.74",
    "input_tp" : "-3.10",
    "input_lra" : "7.30",
    "input_thresh" : "-29.92",
    "output_i" : "-15.99",
    "output_tp" : "-1.50",
    "output_lra" : "5.20",
    "output_thresh" : "-26.10",
    "normalization_type" : "dynamic",
    "target_offset" : "0.40"
}
"#;

#[test]
fn orchestrator_runs_measure_then_apply_and_returns_pass2_output() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_master_loudnorm(dir.path());

    let pass1_out = RenderRunnerOutput {
        stderr: PASS1_STDERR.to_string(),
        output_path: dir.path().join("renders/null"),
        exit_code: Some(0),
    };
    let pass2_out = RenderRunnerOutput {
        stderr: "pass2 ok".to_string(),
        output_path: dir.path().join("renders/final.mp4"),
        exit_code: Some(0),
    };
    let runner = MockRunner::new(vec![Ok(pass1_out), Ok(pass2_out.clone())]);

    let result = run_master_loudnorm_render(dir.path(), &runner).unwrap();

    // Final output is pass-2's output.
    assert_eq!(result.output_path, pass2_out.output_path);
    assert_eq!(result.stderr, pass2_out.stderr);

    let submitted = runner.submitted();
    assert_eq!(submitted.len(), 2, "expected two render submissions");

    // Pass 1: measure
    let pass1_cmd = submitted[0].args.join(" ");
    assert!(
        pass1_cmd.contains("print_format=json"),
        "pass 1 must request JSON output: {pass1_cmd}"
    );
    assert!(
        pass1_cmd.contains("loudnorm=I=-16:LRA=11:TP=-1.5"),
        "pass 1 must include target loudnorm filter: {pass1_cmd}"
    );
    // Null sink: `-f null -`
    let mut found_null_sink = false;
    for (i, w) in submitted[0].args.iter().enumerate() {
        if w == "-f" && submitted[0].args.get(i + 1).map(|s| s.as_str()) == Some("null") {
            found_null_sink = true;
            break;
        }
    }
    assert!(found_null_sink, "pass 1 must use null muxer: {pass1_cmd}");
    assert!(
        !pass1_cmd.contains("libx264"),
        "pass 1 must skip the video encoder: {pass1_cmd}"
    );

    // Pass 2: apply, with parsed measurements substituted in.
    let pass2_cmd = submitted[1].args.join(" ");
    assert!(
        pass2_cmd.contains("measured_I=-19.74"),
        "pass 2 must substitute measured_I from parsed JSON: {pass2_cmd}"
    );
    assert!(
        pass2_cmd.contains("measured_LRA=7.3"),
        "pass 2 must substitute measured_LRA from parsed JSON: {pass2_cmd}"
    );
    assert!(
        pass2_cmd.contains("measured_TP=-3.1"),
        "pass 2 must substitute measured_TP from parsed JSON: {pass2_cmd}"
    );
    assert!(
        pass2_cmd.contains("measured_thresh=-29.92"),
        "pass 2 must substitute measured_thresh from parsed JSON: {pass2_cmd}"
    );
    assert!(
        pass2_cmd.contains("offset=0.4"),
        "pass 2 must substitute target_offset from parsed JSON: {pass2_cmd}"
    );
    assert!(
        pass2_cmd.contains("linear=true"),
        "pass 2 must request linear normalization: {pass2_cmd}"
    );
    assert!(
        pass2_cmd.contains("libx264"),
        "pass 2 keeps the normal video encoder: {pass2_cmd}"
    );
}

#[test]
fn orchestrator_errors_when_pass1_stderr_has_no_json_block() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_master_loudnorm(dir.path());

    let pass1_out = RenderRunnerOutput {
        stderr: "ffmpeg log without loudnorm json".to_string(),
        output_path: dir.path().join("renders/null"),
        exit_code: Some(0),
    };
    let runner = MockRunner::new(vec![Ok(pass1_out)]);

    let err = run_master_loudnorm_render(dir.path(), &runner).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("loudnorm measurement JSON not found") || msg.contains("MeasureJsonNotFound"),
        "expected MeasureJsonNotFound, got: {msg}"
    );
    // Pass 2 must not have been submitted.
    assert_eq!(runner.submitted().len(), 1);
}
