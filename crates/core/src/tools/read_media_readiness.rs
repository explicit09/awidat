//! `read_media_readiness` tool - read-only media/cache/index readiness.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use awidat_index::sidecar_path;
use awidat_proto::index::AssetId;
use serde::Deserialize;
use serde::Serialize;

use crate::FunctionCallError;
use crate::preview_cache::{PreviewArtifactStatus, PreviewCacheEntry};
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Agent-facing media readiness inspection.
pub struct ReadMediaReadinessTool;

#[derive(Debug, Deserialize)]
struct ReadMediaReadinessArgs {
    /// Optional project-relative source asset id, e.g. raw/interview.mov.
    #[serde(default)]
    asset_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct MediaReadinessResponse {
    status: &'static str,
    project_root: String,
    asset_count: usize,
    ready_asset_count: usize,
    pending_asset_count: usize,
    blocked_asset_count: usize,
    summary_for_agent: String,
    entries: Vec<MediaReadinessEntry>,
    recommended_actions: Vec<String>,
}

#[derive(Debug, Serialize)]
struct MediaReadinessEntry {
    asset_id: String,
    source_path: String,
    source_exists: bool,
    state: &'static str,
    state_reason: &'static str,
    playable_artifact: PlayableArtifact,
    cache: MediaCacheReadiness,
    index_signals: Vec<IndexSignalReadiness>,
    agent_usable: AgentUsableSignals,
    recommended_actions: Vec<String>,
}

#[derive(Debug, Serialize)]
struct PlayableArtifact {
    kind: &'static str,
    path: Option<String>,
    backend: &'static str,
    status: &'static str,
    failure_reason: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct MediaCacheReadiness {
    proxy: CacheArtifactReadiness,
    thumbnails: CacheArtifactReadiness,
    waveform: CacheArtifactReadiness,
}

#[derive(Debug, Serialize)]
struct CacheArtifactReadiness {
    status: &'static str,
    path: String,
}

#[derive(Debug, Serialize)]
struct IndexSignalReadiness {
    signal: &'static str,
    indexer: &'static str,
    status: &'static str,
    path: String,
    stale: bool,
    summary: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct AgentUsableSignals {
    transcript: bool,
    word_timestamps: bool,
    speaker_labels: bool,
    scenes: bool,
    audio: bool,
    visual: bool,
}

#[async_trait]
impl ToolHandler for ReadMediaReadinessTool {
    fn name(&self) -> &'static str {
        "read_media_readiness"
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
                        "description": "Optional project-relative source asset id, such as raw/interview.mov. Omit to inspect every raw project asset."
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
        let args: ReadMediaReadinessArgs =
            serde_json::from_value(invocation.args).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "read_media_readiness: invalid args ({e}). Expected optional asset_id string."
                ))
            })?;

        let summary = crate::preview_cache::build_preview_cache_summary(&ctx.project_root)
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "read_media_readiness: unable to scan project media: {e}"
                ))
            })?;
        let mut entries = Vec::new();
        for entry in &summary.entries {
            if args
                .asset_id
                .as_ref()
                .is_some_and(|asset_id| asset_id != &entry.asset_id)
            {
                continue;
            }
            entries.push(read_entry(&ctx.project_root, entry));
        }

        if let Some(asset_id) = args.asset_id.as_deref()
            && entries.is_empty()
        {
            return Err(FunctionCallError::RespondToModel(format!(
                "read_media_readiness: asset '{asset_id}' was not found under project raw media. Use list_assets/view_episode to discover valid asset ids or import/relink the source."
            )));
        }

        let ready_asset_count = entries
            .iter()
            .filter(|entry| entry.state == "ready")
            .count();
        let pending_asset_count = entries
            .iter()
            .filter(|entry| entry.state == "pending")
            .count();
        let blocked_asset_count = entries
            .iter()
            .filter(|entry| entry.state == "blocked")
            .count();
        let recommended_actions = aggregate_actions(&entries);
        let response = MediaReadinessResponse {
            status: if blocked_asset_count > 0 {
                "blocked"
            } else if pending_asset_count > 0 {
                "pending"
            } else {
                "ready"
            },
            project_root: ctx.project_root.to_string_lossy().into_owned(),
            asset_count: entries.len(),
            ready_asset_count,
            pending_asset_count,
            blocked_asset_count,
            summary_for_agent: summarize_for_agent(
                entries.len(),
                ready_asset_count,
                pending_asset_count,
                blocked_asset_count,
            ),
            entries,
            recommended_actions,
        };
        let body = serde_json::to_string(&response).map_err(|e| {
            FunctionCallError::Fatal(format!("read_media_readiness serialization failed: {e}"))
        })?;
        Ok(ToolOutput::text(body))
    }
}

fn read_entry(project_root: &Path, entry: &PreviewCacheEntry) -> MediaReadinessEntry {
    let source_path = PathBuf::from(&entry.asset_path);
    let source_exists = source_path.is_file();
    let indexes = index_signals(project_root, &entry.asset_id, &source_path);
    let agent_usable = agent_usable(&indexes);
    let cache = MediaCacheReadiness {
        proxy: CacheArtifactReadiness {
            status: cache_status(entry.proxy),
            path: entry.proxy_path.clone(),
        },
        thumbnails: CacheArtifactReadiness {
            status: cache_status(entry.thumbnails),
            path: entry.thumbnails_dir.clone(),
        },
        waveform: CacheArtifactReadiness {
            status: cache_status(entry.waveform),
            path: entry.waveform_path.clone(),
        },
    };
    let playable_artifact = playable_artifact(entry, source_exists);
    let mut recommended_actions = Vec::new();
    if !source_exists {
        recommended_actions.push("restore_or_relink_source_media".into());
    }
    if entry.proxy != PreviewArtifactStatus::Fresh {
        recommended_actions.push("refresh_proxy_for_editor_preview".into());
    }
    if !agent_usable.transcript {
        recommended_actions.push("run_start_indexing_with_whisper".into());
    }
    if !agent_usable.scenes {
        recommended_actions.push("run_start_indexing_with_scenedetect".into());
    }
    if !agent_usable.audio {
        recommended_actions.push("run_start_indexing_with_audio_energy".into());
    }

    let (state, state_reason) = if !source_exists {
        ("blocked", "source_missing")
    } else if entry.proxy != PreviewArtifactStatus::Fresh || !agent_usable.transcript {
        ("pending", "cache_or_index_missing")
    } else {
        ("ready", "source_proxy_and_transcript_ready")
    };

    MediaReadinessEntry {
        asset_id: entry.asset_id.clone(),
        source_path: entry.asset_path.clone(),
        source_exists,
        state,
        state_reason,
        playable_artifact,
        cache,
        index_signals: indexes,
        agent_usable,
        recommended_actions,
    }
}

fn playable_artifact(entry: &PreviewCacheEntry, source_exists: bool) -> PlayableArtifact {
    match entry.proxy {
        PreviewArtifactStatus::Fresh => PlayableArtifact {
            kind: "proxy",
            path: Some(entry.proxy_path.clone()),
            backend: "webkit",
            status: "ready",
            failure_reason: None,
        },
        PreviewArtifactStatus::Stale => PlayableArtifact {
            kind: "proxy",
            path: Some(entry.proxy_path.clone()),
            backend: "webkit",
            status: "stale",
            failure_reason: Some("proxy_stale"),
        },
        PreviewArtifactStatus::Missing => PlayableArtifact {
            kind: if source_exists {
                "source"
            } else {
                "unavailable"
            },
            path: source_exists.then(|| entry.asset_path.clone()),
            backend: "webkit",
            status: if source_exists { "fallback" } else { "missing" },
            failure_reason: if source_exists {
                Some("proxy_missing")
            } else {
                Some("source_missing")
            },
        },
        PreviewArtifactStatus::Ready | PreviewArtifactStatus::Empty => PlayableArtifact {
            kind: "source",
            path: source_exists.then(|| entry.asset_path.clone()),
            backend: "webkit",
            status: "fallback",
            failure_reason: Some("proxy_missing"),
        },
    }
}

fn index_signals(
    project_root: &Path,
    asset_id: &str,
    source_path: &Path,
) -> Vec<IndexSignalReadiness> {
    INDEX_SIGNALS
        .iter()
        .map(|signal| read_index_signal(project_root, asset_id, source_path, signal))
        .collect()
}

fn read_index_signal(
    project_root: &Path,
    asset_id: &str,
    source_path: &Path,
    signal: &IndexSignal,
) -> IndexSignalReadiness {
    let asset = AssetId::new(asset_id.to_string());
    let path = sidecar_path(project_root, signal.indexer, &asset)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| {
            project_root
                .join("index")
                .join(signal.indexer)
                .join(format!("{asset_id}.json"))
                .to_string_lossy()
                .into_owned()
        });
    let sidecar_path = PathBuf::from(&path);
    let stale = is_stale(source_path, &sidecar_path);
    let (status, summary) = match std::fs::read(&sidecar_path) {
        Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(value) => (
                if stale { "stale" } else { "ready" },
                summarize_sidecar(signal.name, &value),
            ),
            Err(e) => ("failed", serde_json::json!({ "error": e.to_string() })),
        },
        Err(_) => ("missing", serde_json::json!({})),
    };
    IndexSignalReadiness {
        signal: signal.name,
        indexer: signal.indexer,
        status,
        path,
        stale,
        summary,
    }
}

fn summarize_sidecar(signal: &str, sidecar: &serde_json::Value) -> serde_json::Value {
    let data = sidecar.get("data").unwrap_or(sidecar);
    match signal {
        "transcript" => {
            let segments = data
                .get("segments")
                .and_then(|value| value.as_array())
                .map_or(0, Vec::len);
            let words = data
                .get("words")
                .and_then(|value| value.as_array())
                .map_or(0, Vec::len);
            let speakers = data
                .get("speakers")
                .and_then(|value| value.as_array())
                .map_or(0, Vec::len);
            serde_json::json!({
                "segments": segments,
                "words": words,
                "speakers": speakers,
                "language": data.get("language"),
                "word_timestamps": words > 0,
                "speaker_labels": speakers > 0,
            })
        }
        "scenes" => count_array_summary(data, "shots"),
        "audio_analysis" => serde_json::json!({
            "duration_s": data.get("duration_s"),
            "silences": data.get("silences").and_then(|v| v.as_array()).map_or(0, Vec::len),
            "loudness_integrated_lufs": data.get("loudness_integrated_lufs"),
        }),
        "topics" => count_array_summary(data, "topics"),
        "editorial_moments" => count_array_summary(data, "moments"),
        "color" => count_array_summary(data, "per_frame"),
        "clip_search" => serde_json::json!({
            "frame_count": data.get("frame_count"),
            "has_embeddings": data.get("embeddings_b64").and_then(|v| v.as_str()).is_some(),
        }),
        "face" => count_array_summary(data, "faces"),
        "gaze" | "shot" | "composition" | "frame_quality" => serde_json::json!({
            "frame_count": data.get("frame_count"),
            "summary": data.get("summary"),
        }),
        "generated_description" => serde_json::json!({
            "job_id": data.get("job_id"),
            "visual_summary": data.get("visual_summary"),
            "requires_disclosure": data.get("requires_disclosure"),
            "uses_likeness": data.get("uses_likeness"),
            "provenance": data.get("provenance"),
        }),
        _ => serde_json::json!({}),
    }
}

fn count_array_summary(data: &serde_json::Value, key: &str) -> serde_json::Value {
    serde_json::json!({
        format!("{key}_count"): data.get(key).and_then(|v| v.as_array()).map_or(0, Vec::len)
    })
}

fn agent_usable(indexes: &[IndexSignalReadiness]) -> AgentUsableSignals {
    let transcript = ready(indexes, "transcript");
    AgentUsableSignals {
        transcript,
        word_timestamps: transcript_summary_bool(indexes, "word_timestamps"),
        speaker_labels: transcript_summary_bool(indexes, "speaker_labels"),
        scenes: ready(indexes, "scenes"),
        audio: ready(indexes, "audio_analysis"),
        visual: ["shot", "face", "gaze", "clip_search"]
            .iter()
            .any(|signal| ready(indexes, signal)),
    }
}

fn ready(indexes: &[IndexSignalReadiness], signal: &str) -> bool {
    indexes
        .iter()
        .any(|entry| entry.signal == signal && entry.status == "ready")
}

fn transcript_summary_bool(indexes: &[IndexSignalReadiness], key: &str) -> bool {
    indexes
        .iter()
        .find(|entry| entry.signal == "transcript" && entry.status == "ready")
        .and_then(|entry| entry.summary.get(key))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn cache_status(status: PreviewArtifactStatus) -> &'static str {
    match status {
        PreviewArtifactStatus::Fresh | PreviewArtifactStatus::Ready => "ready",
        PreviewArtifactStatus::Stale => "stale",
        PreviewArtifactStatus::Missing => "missing",
        PreviewArtifactStatus::Empty => "empty",
    }
}

fn is_stale(source_path: &Path, sidecar_path: &Path) -> bool {
    let Ok(source_modified) = source_path.metadata().and_then(|meta| meta.modified()) else {
        return false;
    };
    let Ok(sidecar_modified) = sidecar_path.metadata().and_then(|meta| meta.modified()) else {
        return false;
    };
    sidecar_modified < source_modified
}

fn aggregate_actions(entries: &[MediaReadinessEntry]) -> Vec<String> {
    let mut actions = Vec::new();
    for entry in entries {
        for action in &entry.recommended_actions {
            if !actions.contains(action) {
                actions.push(action.clone());
            }
        }
    }
    actions
}

fn summarize_for_agent(total: usize, ready: usize, pending: usize, blocked: usize) -> String {
    if total == 0 {
        return "No raw media assets were found. Import media before relying on timeline or index data.".into();
    }
    if blocked > 0 {
        return format!(
            "{blocked} of {total} asset(s) are blocked. Repair source paths before trusting playback or indexes."
        );
    }
    if pending > 0 {
        return format!(
            "{ready} of {total} asset(s) are fully ready; {pending} still need proxy or index refresh before the agent should rely on all signals."
        );
    }
    format!("{ready} of {total} asset(s) are ready for agent decisions.")
}

struct IndexSignal {
    name: &'static str,
    indexer: &'static str,
}

const INDEX_SIGNALS: &[IndexSignal] = &[
    IndexSignal {
        name: "transcript",
        indexer: "whisper",
    },
    IndexSignal {
        name: "scenes",
        indexer: "scenedetect",
    },
    IndexSignal {
        name: "audio_analysis",
        indexer: "audio-energy",
    },
    IndexSignal {
        name: "topics",
        indexer: "topic",
    },
    IndexSignal {
        name: "editorial_moments",
        indexer: "editorial-moments",
    },
    IndexSignal {
        name: "color",
        indexer: "color-analysis",
    },
    IndexSignal {
        name: "clip_search",
        indexer: "clip",
    },
    IndexSignal {
        name: "face",
        indexer: "face",
    },
    IndexSignal {
        name: "gaze",
        indexer: "gaze",
    },
    IndexSignal {
        name: "shot",
        indexer: "shot",
    },
    IndexSignal {
        name: "composition",
        indexer: "composition",
    },
    IndexSignal {
        name: "frame_quality",
        indexer: "frame-quality",
    },
    IndexSignal {
        name: "generated_description",
        indexer: "generated-description",
    },
];

const DESCRIPTION: &str = "\
Read media readiness for the current project without side effects. Returns \
source existence, playable artifact choice, proxy/thumbnail/waveform cache \
state, index sidecar availability, and recommended next actions so the agent \
knows whether it can rely on transcript, word timings, speaker labels, scenes, \
audio, visual evidence, or whether it should ask for repair/indexing first.";

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::broadcast;

    use super::*;
    use crate::tool::{SandboxMode, ToolContext, ToolInvocation};

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "read_media_readiness".into(),
            args,
        }
    }

    fn write(path: &Path, body: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn write_sidecar(root: &Path, indexer: &str, asset: &str, data: serde_json::Value) {
        write(
            &root
                .join("index")
                .join(indexer)
                .join(format!("{asset}.json")),
            serde_json::to_string(&serde_json::json!({
                "indexer": indexer,
                "asset_id": asset,
                "data": data,
            }))
            .unwrap()
            .as_bytes(),
        );
    }

    #[tokio::test]
    async fn reports_missing_proxy_and_indexes_as_pending() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("raw/a.mov"), b"source");

        let out = ReadMediaReadinessTool
            .handle(invoke(serde_json::json!({})), ctx_at(dir.path()))
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();

        assert_eq!(body["status"], "pending");
        assert_eq!(body["asset_count"], 1);
        assert_eq!(body["entries"][0]["playable_artifact"]["kind"], "source");
        assert_eq!(body["entries"][0]["cache"]["proxy"]["status"], "missing");
        assert_eq!(body["entries"][0]["agent_usable"]["transcript"], false);
        assert!(
            body["recommended_actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action == "run_start_indexing_with_whisper")
        );
        assert!(!ReadMediaReadinessTool.is_mutating(&invoke(serde_json::json!({}))));
    }

    #[tokio::test]
    async fn reports_ready_proxy_and_agent_signals() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("raw/a.mov");
        write(&asset, b"source");
        let proxy_path = crate::proxy::proxy_path_for(dir.path(), &asset);
        write(&proxy_path, b"proxy");
        write_sidecar(
            dir.path(),
            "whisper",
            "raw/a.mov",
            serde_json::json!({
                "language": "en",
                "speakers": ["SPEAKER_00"],
                "segments": [{"start": 0.0, "end": 1.0, "text": "Hello"}],
                "words": [{"start": 0.0, "end": 0.5, "word": "Hello", "speaker": "SPEAKER_00"}]
            }),
        );
        write_sidecar(
            dir.path(),
            "scenedetect",
            "raw/a.mov",
            serde_json::json!({ "shots": [{"start": 0.0, "end": 1.0}] }),
        );
        write_sidecar(
            dir.path(),
            "audio-energy",
            "raw/a.mov",
            serde_json::json!({ "duration_s": 1.0, "silences": [] }),
        );

        let out = ReadMediaReadinessTool
            .handle(
                invoke(serde_json::json!({ "asset_id": "raw/a.mov" })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();

        assert_eq!(body["status"], "ready");
        assert_eq!(body["ready_asset_count"], 1);
        assert_eq!(body["entries"][0]["playable_artifact"]["kind"], "proxy");
        assert_eq!(body["entries"][0]["agent_usable"]["transcript"], true);
        assert_eq!(body["entries"][0]["agent_usable"]["word_timestamps"], true);
        assert_eq!(body["entries"][0]["agent_usable"]["speaker_labels"], true);
        assert_eq!(body["entries"][0]["agent_usable"]["scenes"], true);
        assert_eq!(body["entries"][0]["agent_usable"]["audio"], true);
    }
}
