//! `stream_remux` tool — stream-copy/remux media through a durable contract.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use awidat_proto::professional::{StreamExportContract, StreamExportMode, StreamExportSpec};
use awidat_render::{OutputPathPolicy, RenderJobSpec, validate_render_output_path};
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Start a stream-copy/remux export job for explicit source streams.
pub struct StreamRemuxTool;

#[derive(Debug, Deserialize)]
struct StreamRemuxArgs {
    /// Project-relative input media path.
    input: String,
    /// Project-relative output media path.
    output: String,
    /// FFmpeg muxer/container name, e.g. `matroska`, `mp4`, or `mov`.
    container: String,
    /// Explicit stream mappings in output order.
    streams: Vec<StreamExportSpec>,
    /// Optional global output metadata.
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

#[async_trait]
impl ToolHandler for StreamRemuxTool {
    fn name(&self) -> &'static str {
        "stream_remux"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "stream_remux".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["input", "output", "container", "streams"],
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "Project-relative source media path."
                    },
                    "output": {
                        "type": "string",
                        "description": "Project-relative output path, usually under renders/remux/."
                    },
                    "container": {
                        "type": "string",
                        "description": "FFmpeg muxer/container name such as matroska, mp4, or mov."
                    },
                    "streams": {
                        "type": "array",
                        "minItems": 1,
                        "description": "Explicit stream mappings in output order.",
                        "items": {
                            "type": "object",
                            "required": ["id", "source_index"],
                            "properties": {
                                "id": {"type": "string"},
                                "kind": {
                                    "type": "string",
                                    "enum": ["video", "audio", "subtitle", "data"],
                                    "default": "video"
                                },
                                "source_index": {
                                    "type": "integer",
                                    "minimum": 0,
                                    "description": "Zero-based source stream index as reported by ffprobe/FFmpeg."
                                },
                                "mode": {
                                    "type": "string",
                                    "enum": ["copy", "transcode"],
                                    "default": "copy"
                                },
                                "codec": {"type": "string"},
                                "language": {"type": "string"},
                                "disposition": {
                                    "type": "array",
                                    "items": {"type": "string"}
                                },
                                "metadata": {
                                    "type": "object",
                                    "additionalProperties": {"type": "string"}
                                }
                            }
                        }
                    },
                    "metadata": {
                        "type": "object",
                        "additionalProperties": {"type": "string"}
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        let input = invocation
            .args
            .get("input")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let output = invocation
            .args
            .get("output")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        vec![ApprovalKey::new(
            self.name(),
            format!("input:{input}:output:{output}:writes=remux"),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: StreamRemuxArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "stream_remux: invalid args ({e}). Required: input, output, container, streams."
            ))
        })?;
        let input_rel = parse_project_relative_path("input", &args.input)?;
        let output_rel = parse_project_relative_path("output", &args.output)?;
        let input_path = ctx.project_root.join(&input_rel);
        let output_path = ctx.project_root.join(&output_rel);
        if !input_path.is_file() {
            return Err(FunctionCallError::RespondToModel(format!(
                "stream_remux: input does not exist: {}",
                input_rel.display()
            )));
        }
        if let Some(parent) = output_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "stream_remux: failed to create output parent {}: {e}",
                    parent.display()
                ))
            })?;
        }
        validate_render_output_path(
            &ctx.project_root,
            &output_path,
            std::slice::from_ref(&input_path),
            &[],
            OutputPathPolicy::default(),
        )
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "stream_remux: output path preflight failed: {e}"
            ))
        })?;

        let contract = StreamExportContract {
            id: format!("stream-remux-{}", stable_slug(&args.input)),
            container: args.container,
            streams: args.streams,
            metadata: args.metadata,
        };
        let argv = awidat_render::professional::plan_stream_export_args(
            &input_path,
            &contract,
            &output_path,
        )
        .map_err(|e| FunctionCallError::RespondToModel(format!("stream_remux: {e}")))?;
        let manifest = build_stream_remux_manifest(StreamRemuxManifestInput {
            project_root: &ctx.project_root,
            input_path: &input_path,
            output_path: &output_path,
            argv: &argv,
            contract: &contract,
        })?;
        let remux_evidence = manifest.manifest.metadata.clone();
        let render_metadata = manifest.manifest.metadata.clone();
        awidat_render::write_render_manifest(&manifest.manifest_path, &manifest.manifest).map_err(
            |e| {
                FunctionCallError::RespondToModel(format!(
                    "stream_remux: failed to write render manifest {}: {e}",
                    manifest.manifest_path.display()
                ))
            },
        )?;

        let job_id = ctx
            .job_manager
            .start(RenderJobSpec {
                args: argv,
                backend: awidat_render::RenderBackendKind::StreamExportRemux,
                total_duration_s: None,
                cwd: Some(ctx.project_root.clone()),
                output_path: output_path.clone(),
                input_paths: vec![input_path],
                manifest_path: Some(manifest.manifest_path.clone()),
                limitations: Vec::new(),
                metadata: render_metadata,
            })
            .await
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "stream_remux: failed to start ffmpeg remux: {e}"
                ))
            })?;

        Ok(ToolOutput::text(
            serde_json::json!({
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
            .to_string(),
        ))
    }
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
    manifest: awidat_render::RenderExecutionManifest,
}

fn build_stream_remux_manifest(
    input: StreamRemuxManifestInput<'_>,
) -> Result<BuiltStreamRemuxManifest, FunctionCallError> {
    let ffmpeg_path = awidat_render::ffmpeg_path().map_err(|e| {
        FunctionCallError::RespondToModel(format!("stream_remux: failed to locate ffmpeg: {e}"))
    })?;
    let mut replay_argv = vec![ffmpeg_path.to_string_lossy().into_owned()];
    replay_argv.extend(input.argv.iter().cloned());
    let project_path = input.project_root.join("project.otio.json");
    let project_hash = optional_file_hash(&project_path)?;
    let metadata = stream_remux_metadata(input.contract);
    let manifest = awidat_render::planned_at_now(awidat_render::RenderExecutionManifestInput {
        created_at: String::new(),
        awidat_version: env!("CARGO_PKG_VERSION").into(),
        project_root: input.project_root.to_string_lossy().into_owned(),
        project_hash,
        timeline_hash: None,
        backend: awidat_render::RenderBackendKind::StreamExportRemux,
        replay: awidat_render::RenderReplayPlan::FfmpegArgv {
            argv: replay_argv,
            cwd: Some(input.project_root.to_string_lossy().into_owned()),
        },
        inputs: vec![
            awidat_render::fingerprint_file(input.input_path, true).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "stream_remux: failed to fingerprint input {}: {e}",
                    input.input_path.display()
                ))
            })?,
        ],
        outputs: vec![awidat_render::output_artifact(input.output_path, true)],
        sidecars: Vec::new(),
        limitations: Vec::new(),
        verification: None,
        metadata,
    });
    Ok(BuiltStreamRemuxManifest {
        manifest_path: awidat_render::manifest_path_for_output(input.output_path),
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
        &awidat_render::RenderBackendKind::StreamExportRemux,
    ));
    metadata
}

fn parse_project_relative_path(kind: &str, raw: &str) -> Result<PathBuf, FunctionCallError> {
    if raw.trim().is_empty() || raw.contains("://") || Path::new(raw).is_absolute() {
        return Err(FunctionCallError::RespondToModel(format!(
            "stream_remux: {kind} must be a safe project-relative path"
        )));
    }
    let path = Path::new(raw);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(FunctionCallError::RespondToModel(format!(
            "stream_remux: {kind} must be a safe project-relative path"
        )));
    }
    Ok(path.to_path_buf())
}

fn optional_file_hash(path: &Path) -> Result<Option<String>, FunctionCallError> {
    if !path.is_file() {
        return Ok(None);
    }
    let fingerprint = awidat_render::fingerprint_file(path, true).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "stream_remux: failed to fingerprint project file {}: {e}",
            path.display()
        ))
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

const DESCRIPTION: &str = "\
Start a first-class stream-copy/remux export. Use this for simple container \
changes, stream extraction/reordering, subtitle/audio passthrough, or other \
packet-preserving jobs that do not need timeline effects, overlays, transitions, \
retiming, or re-encoding. Streams are explicit so the manifest records the exact \
mapping and can be replayed deterministically.";

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::professional::{StreamExportSpec, StreamKind};

    #[test]
    fn stream_remux_metadata_records_mixed_stream_contracts() {
        let contract = StreamExportContract {
            id: "mixed".into(),
            container: "mp4".into(),
            streams: vec![
                StreamExportSpec {
                    id: "v0".into(),
                    kind: StreamKind::Video,
                    source_index: 0,
                    mode: StreamExportMode::Copy,
                    codec: None,
                    language: None,
                    disposition: Vec::new(),
                    metadata: BTreeMap::new(),
                },
                StreamExportSpec {
                    id: "a0".into(),
                    kind: StreamKind::Audio,
                    source_index: 1,
                    mode: StreamExportMode::Transcode,
                    codec: Some("aac".into()),
                    language: None,
                    disposition: Vec::new(),
                    metadata: BTreeMap::new(),
                },
            ],
            metadata: BTreeMap::new(),
        };

        let metadata = stream_remux_metadata(&contract);

        assert_eq!(metadata["copy_stream_count"], "1");
        assert_eq!(metadata["transcode_stream_count"], "1");
        assert_eq!(metadata["all_streams_copy"], "false");
        assert_eq!(
            metadata["remux_eligibility_reason"],
            "explicit_mixed_stream_contract"
        );
    }
}
