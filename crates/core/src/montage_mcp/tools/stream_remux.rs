//! `stream_remux` — stream-copy/remux media through a durable
//! contract. Ported from `crates/core/src/tools/stream_remux.rs` to
//! the in-process MCP server.
//!
//! Mutating: spawns an ffmpeg job that writes to the project's
//! `renders/remux/` directory. The original `ToolHandler` had
//! `is_mutating = true` and routed an `ApprovalKey` through
//! `ToolContext.approval_tx`. Both are dropped in the port: codex
//! performs the destructive-hint approval before dispatching the call,
//! and the MCP server constructs a fresh `JobManager` per call (the
//! MCP server is short-lived and the spawned ffmpeg task continues
//! independently).

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use montage_proto::professional::{StreamExportContract, StreamExportMode, StreamExportSpec};
use montage_render::{JobManager, OutputPathPolicy, RenderJobSpec, validate_render_output_path};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `stream_remux`.
///
/// The `streams` field is typed as `serde_json::Value` because
/// `StreamExportSpec` lives in `montage_proto` and does not implement
/// `JsonSchema`. We deserialize into `Vec<StreamExportSpec>` inside
/// [`run`] so the schema stays generic while the runtime still
/// validates the proto shape.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct StreamRemuxArgs {
    /// Project-relative input media path.
    pub input: String,
    /// Project-relative output media path.
    pub output: String,
    /// FFmpeg muxer/container name, e.g. `matroska`, `mp4`, or `mov`.
    pub container: String,
    /// Explicit stream mappings in output order. Each entry must
    /// match the `StreamExportSpec` shape from `montage_proto`:
    /// `{ id, kind?, source_index, mode?, codec?, language?,
    /// disposition?, metadata? }`.
    pub streams: serde_json::Value,
    /// Optional global output metadata.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

/// Run `stream_remux` against the project resolved from
/// [`McpToolCtx`]. Returns a JSON status body as `Ok(String)`;
/// validation / job-start failures return `Err(String)`.
pub async fn run(args: StreamRemuxArgs, ctx: McpToolCtx) -> Result<String, String> {
    let input_rel = parse_project_relative_path("input", &args.input)?;
    let output_rel = parse_project_relative_path("output", &args.output)?;
    let input_path = ctx.project_root.join(&input_rel);
    let output_path = ctx.project_root.join(&output_rel);
    if !input_path.is_file() {
        return Err(format!(
            "stream_remux: input does not exist: {}",
            input_rel.display()
        ));
    }
    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            format!(
                "stream_remux: failed to create output parent {}: {e}",
                parent.display()
            )
        })?;
    }
    validate_render_output_path(
        &ctx.project_root,
        &output_path,
        std::slice::from_ref(&input_path),
        &[],
        OutputPathPolicy::default(),
    )
    .map_err(|e| format!("stream_remux: output path preflight failed: {e}"))?;

    let streams: Vec<StreamExportSpec> = serde_json::from_value(args.streams).map_err(|e| {
        format!("stream_remux: streams must be a list of StreamExportSpec entries ({e})")
    })?;
    let contract = StreamExportContract {
        id: format!("stream-remux-{}", stable_slug(&args.input)),
        container: args.container,
        streams,
        metadata: args.metadata,
    };
    let argv =
        montage_render::professional::plan_stream_export_args(&input_path, &contract, &output_path)
            .map_err(|e| format!("stream_remux: {e}"))?;
    let manifest = build_stream_remux_manifest(StreamRemuxManifestInput {
        project_root: &ctx.project_root,
        input_path: &input_path,
        output_path: &output_path,
        argv: &argv,
        contract: &contract,
    })?;
    let remux_evidence = manifest.manifest.metadata.clone();
    let render_metadata = manifest.manifest.metadata.clone();
    montage_render::write_render_manifest(&manifest.manifest_path, &manifest.manifest).map_err(
        |e| {
            format!(
                "stream_remux: failed to write render manifest {}: {e}",
                manifest.manifest_path.display()
            )
        },
    )?;

    let job_manager = JobManager::new();
    let job_id = job_manager
        .start(RenderJobSpec {
            args: argv,
            backend: montage_render::RenderBackendKind::StreamExportRemux,
            total_duration_s: None,
            cwd: Some(ctx.project_root.clone()),
            output_path: output_path.clone(),
            input_paths: vec![input_path],
            manifest_path: Some(manifest.manifest_path.clone()),
            limitations: Vec::new(),
            metadata: render_metadata,
        })
        .await
        .map_err(|e| format!("stream_remux: failed to start ffmpeg remux: {e}"))?;

    Ok(serde_json::json!({
        "job_id": job_id.to_string(),
        "render_kind": "stream_export_remux",
        "input_path": ctx.project_root.join(input_rel).display().to_string(),
        "output_path": output_path.display().to_string(),
        "manifest_path": manifest.manifest_path.display().to_string(),
        "container": contract.container,
        "stream_count": contract.streams.len(),
        "remux_evidence": remux_evidence,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "next_step": format!("Call poll_render(job_id=\"{job_id}\") to track this stream remux export.")
    })
    .to_string())
}

struct StreamRemuxManifestInput<'a> {
    project_root: &'a Path,
    input_path: &'a Path,
    output_path: &'a Path,
    argv: &'a [String],
    contract: &'a StreamExportContract,
}

struct BuiltStreamRemuxManifest {
    manifest_path: PathBuf,
    manifest: montage_render::RenderExecutionManifest,
}

fn build_stream_remux_manifest(
    input: StreamRemuxManifestInput<'_>,
) -> Result<BuiltStreamRemuxManifest, String> {
    let ffmpeg_path = montage_render::ffmpeg_path()
        .map_err(|e| format!("stream_remux: failed to locate ffmpeg: {e}"))?;
    let mut replay_argv = vec![ffmpeg_path.to_string_lossy().into_owned()];
    replay_argv.extend(input.argv.iter().cloned());
    let project_path = input.project_root.join("project.otio.json");
    let project_hash = optional_file_hash(&project_path)?;
    let metadata = stream_remux_metadata(input.contract);
    let manifest = montage_render::planned_at_now(montage_render::RenderExecutionManifestInput {
        created_at: String::new(),
        montage_version: env!("CARGO_PKG_VERSION").into(),
        project_root: input.project_root.to_string_lossy().into_owned(),
        project_hash,
        timeline_hash: None,
        backend: montage_render::RenderBackendKind::StreamExportRemux,
        replay: montage_render::RenderReplayPlan::FfmpegArgv {
            argv: replay_argv,
            cwd: Some(input.project_root.to_string_lossy().into_owned()),
        },
        inputs: vec![
            montage_render::fingerprint_file(input.input_path, true).map_err(|e| {
                format!(
                    "stream_remux: failed to fingerprint input {}: {e}",
                    input.input_path.display()
                )
            })?,
        ],
        outputs: vec![montage_render::output_artifact(input.output_path, true)],
        sidecars: Vec::new(),
        limitations: Vec::new(),
        verification: None,
        metadata,
    });
    Ok(BuiltStreamRemuxManifest {
        manifest_path: montage_render::manifest_path_for_output(input.output_path),
        manifest,
    })
}

fn stream_remux_metadata(contract: &StreamExportContract) -> BTreeMap<String, String> {
    let copy_stream_count = contract
        .streams
        .iter()
        .filter(|stream| stream.mode == StreamExportMode::Copy)
        .count();
    let transcode_stream_count = contract.streams.len().saturating_sub(copy_stream_count);
    let all_streams_copy = copy_stream_count == contract.streams.len();
    let eligibility_reason = if all_streams_copy {
        "explicit_stream_copy_contract"
    } else {
        "explicit_mixed_stream_contract"
    };
    let mut metadata = BTreeMap::from([
        ("contract_id".into(), contract.id.clone()),
        ("container".into(), contract.container.clone()),
        ("stream_count".into(), contract.streams.len().to_string()),
        ("remux_backend".into(), "stream_export_remux".into()),
        ("remux_eligibility_reason".into(), eligibility_reason.into()),
        ("copy_stream_count".into(), copy_stream_count.to_string()),
        (
            "transcode_stream_count".into(),
            transcode_stream_count.to_string(),
        ),
        ("all_streams_copy".into(), all_streams_copy.to_string()),
    ]);
    metadata.extend(crate::capabilities::render_feature_metadata_for_backend(
        &montage_render::RenderBackendKind::StreamExportRemux,
    ));
    metadata
}

fn parse_project_relative_path(kind: &str, raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() || raw.contains("://") || Path::new(raw).is_absolute() {
        return Err(format!(
            "stream_remux: {kind} must be a safe project-relative path"
        ));
    }
    let path = Path::new(raw);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "stream_remux: {kind} must be a safe project-relative path"
        ));
    }
    Ok(path.to_path_buf())
}

fn optional_file_hash(path: &Path) -> Result<Option<String>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let fingerprint = montage_render::fingerprint_file(path, true).map_err(|e| {
        format!(
            "stream_remux: failed to fingerprint project file {}: {e}",
            path.display()
        )
    })?;
    Ok(Some(fingerprint.sha256))
}

fn stable_slug(value: &str) -> String {
    let mut slug = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

pub const DESCRIPTION: &str = "\
Start a first-class stream-copy/remux export. Use this for simple container \
changes, stream extraction/reordering, subtitle/audio passthrough, or other \
packet-preserving jobs that do not need timeline effects, overlays, transitions, \
retiming, or re-encoding. Streams are explicit so the manifest records the exact \
mapping and can be replayed deterministically.";
