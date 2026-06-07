//! `podcast_episode_spans` tool — wrap the bundled episode span planner.

use std::path::{Path, PathBuf};
use std::process::Command;

use async_trait::async_trait;
use montage_index::{sidecar_path, walk_indexer};
use montage_proto::index::AssetId;
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Read-only episode span planner.
pub struct PodcastEpisodeSpansTool;

#[derive(Debug, Deserialize)]
struct PodcastEpisodeSpansArgs {
    #[serde(default)]
    asset_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct EpisodeSpanReport {
    status: &'static str,
    summary_for_agent: String,
    assets: Vec<AssetSpanReport>,
    missing_evidence: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AssetSpanReport {
    asset_id: String,
    status: String,
    episode_spans: serde_json::Value,
    rejected_spans: serde_json::Value,
    recommended_span: serde_json::Value,
    requires_user_choice: bool,
    evidence: serde_json::Value,
}

#[async_trait]
impl ToolHandler for PodcastEpisodeSpansTool {
    fn name(&self) -> &'static str {
        "podcast_episode_spans"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Optional project-relative asset path. Omit to plan spans for every whisper transcript."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: PodcastEpisodeSpansArgs =
            serde_json::from_value(invocation.args).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "podcast_episode_spans: invalid args ({e}). All fields are optional."
                ))
            })?;

        let script = episode_span_script_path()?;
        let assets = resolve_assets(&ctx.project_root, args.asset_id)?;
        if assets.is_empty() {
            let report = EpisodeSpanReport {
                status: "missing_indexes",
                summary_for_agent:
                    "No whisper transcript sidecars found; cannot plan episode spans.".into(),
                assets: Vec::new(),
                missing_evidence: vec!["whisper transcript sidecar".into()],
            };
            return serialize_report(&report);
        }

        let mut reports = Vec::new();
        let mut missing_evidence = Vec::new();
        for asset_id in assets {
            let transcript = sidecar_path(&ctx.project_root, "whisper", &AssetId::new(&asset_id))
                .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?;
            let audio_energy = optional_sidecar_path(&ctx.project_root, "audio-energy", &asset_id);
            let topic = optional_sidecar_path(&ctx.project_root, "topic", &asset_id);
            if audio_energy.is_none() {
                missing_evidence.push(format!("{asset_id}: missing audio-energy sidecar"));
            }
            if topic.is_none() {
                missing_evidence.push(format!("{asset_id}: missing topic sidecar"));
            }
            reports.push(run_planner(
                &script,
                &asset_id,
                &transcript,
                audio_energy.as_deref(),
                topic.as_deref(),
            )?);
        }

        let requires_choice = reports
            .iter()
            .filter(|report| report.requires_user_choice)
            .count();
        let span_count: usize = reports
            .iter()
            .filter_map(|report| report.episode_spans.as_array().map(Vec::len))
            .sum();
        let status = if reports.is_empty() {
            "missing_indexes"
        } else if missing_evidence.is_empty() {
            "ready"
        } else {
            "partial"
        };
        let summary_for_agent = format!(
            "Episode span status: {status}. Found {span_count} candidate span(s) across {} asset(s); {requires_choice} asset(s) require user choice.",
            reports.len()
        );
        let report = EpisodeSpanReport {
            status,
            summary_for_agent,
            assets: reports,
            missing_evidence,
        };
        serialize_report(&report)
    }
}

fn resolve_assets(
    project_root: &Path,
    asset_id: Option<String>,
) -> Result<Vec<String>, FunctionCallError> {
    if let Some(asset_id) = asset_id {
        let path = sidecar_path(project_root, "whisper", &AssetId::new(&asset_id))
            .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?;
        if !path.exists() {
            return Err(FunctionCallError::RespondToModel(format!(
                "podcast_episode_spans: no whisper sidecar at {}; run the whisper indexer first",
                path.display()
            )));
        }
        return Ok(vec![asset_id]);
    }
    let walker = walk_indexer(project_root, "whisper")
        .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?;
    Ok(walker.map(|(asset, _)| asset).collect())
}

fn optional_sidecar_path(project_root: &Path, indexer: &str, asset_id: &str) -> Option<PathBuf> {
    let path = sidecar_path(project_root, indexer, &AssetId::new(asset_id)).ok()?;
    path.exists().then_some(path)
}

fn run_planner(
    script: &Path,
    asset_id: &str,
    transcript: &Path,
    audio_energy: Option<&Path>,
    topic: Option<&Path>,
) -> Result<AssetSpanReport, FunctionCallError> {
    let mut command = Command::new("python3");
    command.arg(script).arg("--transcript").arg(transcript);
    if let Some(path) = audio_energy {
        command.arg("--audio-energy").arg(path);
    }
    if let Some(path) = topic {
        command.arg("--topic").arg(path);
    }

    let output = command.output().map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "podcast_episode_spans: failed to run {}: {e}",
            script.display()
        ))
    })?;
    if !output.status.success() {
        return Err(FunctionCallError::RespondToModel(format!(
            "podcast_episode_spans: planner failed for {asset_id}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "podcast_episode_spans: planner returned malformed JSON for {asset_id}: {e}"
        ))
    })?;
    Ok(AssetSpanReport {
        asset_id: asset_id.to_string(),
        status: parsed
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("planned")
            .to_string(),
        episode_spans: parsed
            .get("episode_spans")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        rejected_spans: parsed
            .get("rejected_spans")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        recommended_span: parsed
            .get("recommended_span")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        requires_user_choice: parsed
            .get("requires_user_choice")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        evidence: parsed
            .get("evidence")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    })
}

fn episode_span_script_path() -> Result<PathBuf, FunctionCallError> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| FunctionCallError::Fatal("cannot resolve repo root".into()))?;
    let path = repo_root
        .join("skills")
        .join("auto-cutter")
        .join("scripts")
        .join("episode_span_plan.py");
    if path.exists() {
        Ok(path)
    } else {
        Err(FunctionCallError::RespondToModel(format!(
            "podcast_episode_spans: missing planner script at {}",
            path.display()
        )))
    }
}

fn serialize_report(report: &EpisodeSpanReport) -> Result<ToolOutput, FunctionCallError> {
    serde_json::to_string(report)
        .map(ToolOutput::text)
        .map_err(|e| FunctionCallError::Fatal(format!("podcast_episode_spans serialize: {e}")))
}

const DESCRIPTION: &str = "\
Plan candidate episode spans from existing transcript/audio/topic evidence. \
Wraps the bundled auto-cutter episode_span_plan.py script; returns \
recommended start/end spans and whether multiple high-confidence episodes \
require the user to choose before extraction or cleanup.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{SandboxMode, ToolContext, ToolInvocation};
    use montage_render::JobManager;
    use tokio::sync::broadcast;

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: JobManager::new(),
            approval_tx: None,
            sandbox_mode: SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(montage_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn write_sidecar(root: &std::path::Path, indexer: &str, asset: &str, data: serde_json::Value) {
        let p = root
            .join("index")
            .join(indexer)
            .join(format!("{asset}.json"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            p,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": indexer,
                "asset_id": asset,
                "data": data,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn reports_multiple_episode_spans_from_existing_planner() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/episode.mov";
        write_sidecar(
            dir.path(),
            "whisper",
            asset,
            serde_json::json!({
                "segments": [
                    {"start_s": 0.0, "end_s": 5.0, "text": "Pre show setup chat."},
                    {"start_s": 10.0, "end_s": 20.0, "text": "Welcome to the show. In this episode we discuss product validation."},
                    {"start_s": 500.0, "end_s": 510.0, "text": "Thanks for listening and follow us for more."},
                    {"start_s": 700.0, "end_s": 710.0, "text": "Welcome back. Today we have a different topic."},
                    {"start_s": 1200.0, "end_s": 1210.0, "text": "See you next time."}
                ]
            }),
        );
        write_sidecar(
            dir.path(),
            "audio-energy",
            asset,
            serde_json::json!({"silences": []}),
        );
        write_sidecar(
            dir.path(),
            "topic",
            asset,
            serde_json::json!({"topics": []}),
        );

        let out = PodcastEpisodeSpansTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "podcast_episode_spans".into(),
                    args: serde_json::json!({"asset_id": asset}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(value["status"], "ready");
        assert_eq!(value["assets"][0]["requires_user_choice"], true);
        assert_eq!(
            value["assets"][0]["episode_spans"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn rejects_rehearsal_false_starts_before_publishable_episode() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/rehearsal.mov";
        write_sidecar(
            dir.path(),
            "whisper",
            asset,
            serde_json::json!({
                "segments": [
                    {"start_s": 238.0, "end_s": 245.0, "text": "Pre show planning chatter before we start."},
                    {"start_s": 676.445, "end_s": 677.165, "text": "You're welcome."},
                    {"start_s": 1206.615, "end_s": 1220.455, "text": "Hello, everyone. Welcome to Technogia. I'm just practicing. Am I supposed to look at that camera?"},
                    {"start_s": 1293.855, "end_s": 1309.270, "text": "Welcome to Technocia Talks. Today we have a special guest. That was a practice. Cut."},
                    {"start_s": 1350.535, "end_s": 1378.975, "text": "Welcome to Technosia Talks. Today, we have a very interesting guest with us. What's the other one? Hold it for me one last time."},
                    {"start_s": 1463.375, "end_s": 1486.080, "text": "Welcome to Technocia Talks. Today, we have a very special guest with us. Thank you for coming, mister Youssef."},
                    {"start_s": 1500.0, "end_s": 4594.27, "text": "Sustained interview discussion about climate engineering, coral reefs, vertical farming, and the founder story."}
                ]
            }),
        );
        write_sidecar(
            dir.path(),
            "audio-energy",
            asset,
            serde_json::json!({"silences": [{"start_s": 224.0, "end_s": 238.0}]}),
        );
        write_sidecar(
            dir.path(),
            "topic",
            asset,
            serde_json::json!({"topics": []}),
        );

        let out = PodcastEpisodeSpansTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "podcast_episode_spans".into(),
                    args: serde_json::json!({"asset_id": asset}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let asset_report = &value["assets"][0];
        let spans = asset_report["episode_spans"].as_array().unwrap();
        assert_eq!(asset_report["requires_user_choice"], false);
        assert_eq!(spans.len(), 1);
        assert_eq!(asset_report["recommended_span"]["start_s"], 1463.375);
        assert!(asset_report["rejected_spans"].as_array().unwrap().len() >= 4);
    }
}
