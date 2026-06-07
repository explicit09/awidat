//! `read_media_readiness` — read-only media/cache/index readiness.
//! Ported in step 5 from `crates/core/src/tools/read_media_readiness.rs`
//! to the in-process MCP server.

use std::path::{Path, PathBuf};

use montage_index::sidecar_path;
use montage_proto::index::AssetId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::preview_cache::{PreviewArtifactStatus, PreviewCacheEntry};

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ReadMediaReadinessArgs {
    /// Optional project-relative source asset id, e.g. raw/interview.mov.
    #[serde(default)]
    pub asset_id: Option<String>,
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

/// Run `read_media_readiness` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`; scanning or
/// argument errors return `Err(String)`.
pub fn run(args: ReadMediaReadinessArgs, ctx: McpToolCtx) -> Result<String, String> {
    let summary = crate::preview_cache::build_preview_cache_summary(&ctx.project_root)
        .map_err(|e| format!("read_media_readiness: unable to scan project media: {e}"))?;
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
        return Err(format!(
            "read_media_readiness: asset '{asset_id}' was not found under project raw media. Use list_assets/view_episode to discover valid asset ids or import/relink the source."
        ));
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
    serde_json::to_string(&response)
        .map_err(|e| format!("read_media_readiness serialization failed: {e}"))
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

/// Tool description, served to the model via MCP `tools/list`.
pub const DESCRIPTION: &str = "\
Read media readiness for the current project without side effects. Returns \
source existence, playable artifact choice, proxy/thumbnail/waveform cache \
state, index sidecar availability, and recommended next actions so the agent \
knows whether it can rely on transcript, word timings, speaker labels, scenes, \
audio, visual evidence, or whether it should ask for repair/indexing first.";
