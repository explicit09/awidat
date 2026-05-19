use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use awidat_render::{generate_silences, probe_media};
use chrono::Utc;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::fixtures::{ClipSpec, write_project_with_clips};

use super::artifacts::{
    ArtifactBundleInput, RequiredArtifactInput, build_artifact_bundle_manifest,
    required_artifact_entries, required_artifacts_present,
};
use super::case::AcceptanceCase;
use super::judges::{add_media_probe_gates, add_source_range_gates, build_transcript_evidence};
use super::media::{
    detect_black_segments, render_project, silences_to_json, write_ffprobe_json,
    write_real_media_excerpt, write_synthetic_media, write_transcript_sidecar,
};
use super::scorecard::{AcceptanceScorecard, ScorecardInput, build_scorecard, push_gate};
use super::timeline::apply_final_edl;

pub(crate) async fn run_case(case: &AcceptanceCase) -> Result<AcceptanceRun> {
    let run_root = create_run_root(&case.id)?;
    let project_root = run_root.join("project");
    let artifacts_root = run_root.join("artifacts");
    fs::create_dir_all(&artifacts_root)?;

    write_project_with_clips(
        &project_root,
        &case.project.name,
        &[ClipSpec::video(
            "source",
            "source",
            &case.project.source_asset,
            0.0,
            case.project.source_duration_s,
        )],
    )?;
    let local_source = project_root.join(&case.project.source_asset);
    match (&case.synthetic_media, &case.project.source_path) {
        (Some(media), None) => write_synthetic_media(&local_source, media)?,
        (None, Some(source_path)) => write_real_media_excerpt(&local_source, source_path, case)?,
        (Some(_), Some(_)) => {
            bail!("acceptance case must not specify both synthetic_media and project.source_path");
        }
        (None, None) => {
            bail!("acceptance case must specify synthetic_media or project.source_path")
        }
    }
    if let Some(transcript) = &case.expect.transcript {
        write_transcript_sidecar(&project_root, &case.project.source_asset, transcript)
            .context("write acceptance transcript sidecar")?;
    }

    let edit = apply_final_edl(&project_root, &artifacts_root, case)?;
    let final_edl_path = artifacts_root.join("final_edl.edl");
    let generator_stdout_path = edit.generator_stdout_path.as_ref().map(PathBuf::from);
    let generator_stderr_path = edit.generator_stderr_path.as_ref().map(PathBuf::from);
    let edit_manifest_path = artifacts_root.join("edit_manifest.json");
    fs::write(&edit_manifest_path, serde_json::to_vec_pretty(&edit)?)?;

    let mut gates = Vec::new();
    add_source_range_gates(&mut gates, case, &edit);

    let transcript_path = if let Some(transcript) = &case.expect.transcript {
        let evidence = build_transcript_evidence(transcript, &edit.final_kept_source_ranges);
        let path = artifacts_root.join("transcript.json");
        fs::write(&path, serde_json::to_vec_pretty(&evidence)?)?;
        push_gate(
            &mut gates,
            "transcript_must_preserve",
            evidence.preserve_passed,
            "required transcript phrases are present in final kept source ranges",
            json!({
                "transcript_path": path,
                "checks": evidence.preserve_checks,
                "final_kept_text": evidence.final_kept_text,
            }),
        );
        push_gate(
            &mut gates,
            "transcript_must_remove",
            evidence.remove_passed,
            "removed transcript phrases are absent from final kept source ranges",
            json!({
                "transcript_path": path,
                "checks": evidence.remove_checks,
            }),
        );
        Some(path)
    } else {
        None
    };

    let render = render_project(&project_root).context("render scenario timeline")?;
    push_gate(
        &mut gates,
        "render_ok",
        render.output_path.exists(),
        "rendered MP4 exists after timeline export",
        json!({
            "driver": render.driver,
            "output_path": render.output_path,
        }),
    );

    let ffprobe_json_path = artifacts_root.join("ffprobe.json");
    let ffprobe_json = write_ffprobe_json(&render.output_path, &ffprobe_json_path)
        .context("write ffprobe artifact")?;
    let probe = probe_media(&render.output_path)
        .await
        .context("probe rendered output")?;
    add_media_probe_gates(&mut gates, case, &probe);

    let silences = generate_silences(
        &render.output_path,
        case.expect.silence_threshold_db,
        case.expect.max_long_silence_s,
        CancellationToken::new(),
    )
    .await
    .context("detect rendered-output silence")?;
    let silence_path = artifacts_root.join("silence.json");
    fs::write(
        &silence_path,
        serde_json::to_vec_pretty(&silences_to_json(&silences))?,
    )?;
    push_gate(
        &mut gates,
        "no_long_silence",
        silences.is_empty(),
        "no silence range meets or exceeds the configured long-silence threshold",
        json!({
            "threshold_db": case.expect.silence_threshold_db,
            "max_long_silence_s": case.expect.max_long_silence_s,
            "ranges": silences_to_json(&silences),
        }),
    );

    let black_segments = detect_black_segments(
        &render.output_path,
        case.expect.black_min_duration_s,
        case.expect.max_black_segment_s,
    )
    .context("detect rendered-output black frames")?;
    let blackdetect_path = artifacts_root.join("blackdetect.json");
    fs::write(
        &blackdetect_path,
        serde_json::to_vec_pretty(&black_segments)?,
    )?;
    push_gate(
        &mut gates,
        "no_long_black_segment",
        black_segments.is_empty(),
        "no black segment exceeds the configured max duration",
        json!({
            "black_min_duration_s": case.expect.black_min_duration_s,
            "max_black_segment_s": case.expect.max_black_segment_s,
            "segments": black_segments,
        }),
    );

    let scorecard_path = artifacts_root.join("scorecard.json");
    let artifact_bundle_path = artifacts_root.join("artifact_bundle.json");
    let required_artifacts = required_artifact_entries(RequiredArtifactInput {
        output_path: &render.output_path,
        final_edl_path: &final_edl_path,
        edit_manifest_path: &edit_manifest_path,
        ffprobe_json_path: &ffprobe_json_path,
        silence_path: &silence_path,
        blackdetect_path: &blackdetect_path,
        transcript_path: transcript_path.as_deref(),
        generator_stdout_path: generator_stdout_path.as_deref(),
        generator_stderr_path: generator_stderr_path.as_deref(),
    });
    push_gate(
        &mut gates,
        "artifact_bundle_inputs_present",
        required_artifacts_present(&required_artifacts),
        "rendered output and probe artifacts exist before scorecard packaging",
        json!({
            "entries": required_artifacts,
        }),
    );

    push_gate(
        &mut gates,
        "edit_manifest_valid",
        edit.applied_descriptions.len() >= edit.expected_min_applied_ops
            && edit.final_clip_count >= case.expect.min_clip_count,
        "final EDL applied and produced enough timeline evidence for the scenario",
        json!({
            "edit_manifest_path": edit_manifest_path,
            "applied_descriptions": edit.applied_descriptions,
            "final_clip_count": edit.final_clip_count,
            "min_clip_count": case.expect.min_clip_count,
        }),
    );

    let scorecard = build_scorecard(ScorecardInput {
        case,
        run_root: &run_root,
        artifacts_root: &artifacts_root,
        output_path: &render.output_path,
        final_edl_path: &final_edl_path,
        edit_manifest_path: &edit_manifest_path,
        ffprobe_json_path: &ffprobe_json_path,
        silence_path: &silence_path,
        blackdetect_path: &blackdetect_path,
        transcript_path: transcript_path.as_deref(),
        generator_stdout_path: generator_stdout_path.as_deref(),
        generator_stderr_path: generator_stderr_path.as_deref(),
        artifact_bundle_path: &artifact_bundle_path,
        edit_driver: &edit.edit_driver,
        render_driver: &render.driver,
        ffprobe_json,
        gates,
    });
    fs::write(&scorecard_path, serde_json::to_vec_pretty(&scorecard)?)?;
    let artifact_bundle = build_artifact_bundle_manifest(ArtifactBundleInput {
        run_root: &run_root,
        output_path: &render.output_path,
        final_edl_path: &final_edl_path,
        edit_manifest_path: &edit_manifest_path,
        ffprobe_json_path: &ffprobe_json_path,
        silence_path: &silence_path,
        blackdetect_path: &blackdetect_path,
        transcript_path: transcript_path.as_deref(),
        generator_stdout_path: generator_stdout_path.as_deref(),
        generator_stderr_path: generator_stderr_path.as_deref(),
        scorecard_path: &scorecard_path,
    });
    fs::write(
        &artifact_bundle_path,
        serde_json::to_vec_pretty(&artifact_bundle)?,
    )?;

    Ok(AcceptanceRun {
        scorecard,
        scorecard_path,
    })
}

pub(crate) fn create_run_root(scenario_id: &str) -> Result<PathBuf> {
    static RUN_ROOT_COUNTER: AtomicU64 = AtomicU64::new(0);

    let base = std::env::var("AWIDAT_EVAL_ARTIFACTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from("target")
                .join("awidat-eval")
                .join("acceptance")
        });
    let base = if base.is_absolute() {
        base
    } else {
        std::env::current_dir()
            .context("resolve current directory for acceptance artifacts")?
            .join(base)
    };
    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let scenario_root = base.join(sanitize_path_segment(scenario_id));
    fs::create_dir_all(&scenario_root)?;
    for _ in 0..100 {
        let counter = RUN_ROOT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let run_root = scenario_root.join(format!(
            "{}-p{}-{counter:04}",
            timestamp,
            std::process::id()
        ));
        match fs::create_dir(&run_root) {
            Ok(()) => return Ok(run_root),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create acceptance run root"),
        }
    }
    anyhow::bail!(
        "failed to create a unique acceptance run root under {}",
        scenario_root.display()
    )
}

fn sanitize_path_segment(raw: &str) -> String {
    raw.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}
#[derive(Debug)]
pub(crate) struct AcceptanceRun {
    pub(crate) scorecard: AcceptanceScorecard,
    pub(crate) scorecard_path: PathBuf,
}
