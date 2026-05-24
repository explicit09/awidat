//! Transcript pane backend (Step 6). Reads the project's whisper
//! sidecar for one asset and returns it in the protocol shape the
//! frontend renders directly.
//!
//! Sidecar layout: `<project>/index/whisper/<asset_id>.json`. Shape
//! (per `python/packages/whisper-mcp/src/whisper_mcp/__init__.py`):
//!
//! ```json
//! {
//!   "indexer": "whisper",
//!   "indexer_version": "...",
//!   "schema_version": "...",
//!   "asset_id": "raw/...",
//!   "asset_sha256": "...",
//!   "produced_at": "...",
//!   "data": {
//!     "language": "en",
//!     "model": "...",
//!     "diarized": true,
//!     "words":    [{"text": "...", "start_s": ..., "end_s": ..., "speaker_id": "..."}],
//!     "segments": [{"text": "...", "start_s": ..., "end_s": ..., "speaker_id": "..."}],
//!     "speakers": [{"id": "...", "total_speech_s": ...}]
//!   }
//! }
//! ```
//!
//! Caching: `AwidatState::transcript_cache` holds parsed `Transcript`
//! values keyed by stem. The cache is invalidated on indexer-job
//! Completed (the run-loop emits a `JobKind::Index` event that the
//! frontend can listen to and re-call `read_transcript`). For v1 we
//! keep the cache simple and don't auto-invalidate on file mtime
//! changes — the indexer Completed signal is the single trigger.

use std::path::Path;

use awidat_desktop_protocol::{Transcript, TranscriptSegment, TranscriptSpeaker, TranscriptWord};
use awidat_proto::index::AssetId;
use serde::Deserialize;
use tauri::State;

use crate::commands::media::proxy_path_for;
use crate::state::AwidatState;

/// Read the whisper transcript for the asset whose proxy stem is
/// `stem`. Returns `None` when:
///   - no project loaded
///   - no asset on disk maps to that stem
///   - the asset has no whisper sidecar yet (transcoded but not
///     indexed)
///
/// The frontend treats `None` as "transcript not available" and
/// shows a placeholder; once the indexer Completes the frontend
/// re-calls and the result populates.
#[tauri::command]
pub async fn read_transcript(
    state: State<'_, AwidatState>,
    stem: String,
) -> Result<Option<Transcript>, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(None),
    };

    // Cache check first — a 4 MB transcript shouldn't be re-parsed
    // on every tab toggle.
    if let Some(hit) = state.transcript_cache.lock().await.get(&stem) {
        return Ok(Some(hit.clone()));
    }

    // Resolve stem → asset_id. The asset's proxy filename is
    // `<asset-stem>-<8hex>.mp4`; the stem we receive is that filename
    // minus `.mp4`. Walk raw/ and recompute each candidate's proxy
    // stem, matching against ours.
    let stem_clone = stem.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<Option<Transcript>, String> {
        let asset_abs = match resolve_asset_for_stem(&project_root, &stem_clone) {
            Some(p) => p,
            None => return Ok(None),
        };
        let asset_id_rel = asset_abs
            .strip_prefix(&project_root)
            .map_err(|_| format!("asset {} not under project root", asset_abs.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let asset_id = AssetId::new(asset_id_rel);
        match awidat_index::read_sidecar(&project_root, "whisper", &asset_id) {
            Ok(value) => Ok(Some(parse_transcript(stem_clone, value)?)),
            Err(awidat_index::SidecarError::NotFound { .. }) => Ok(None),
            Err(e) => Err(format!("read whisper sidecar: {e}")),
        }
    })
    .await
    .map_err(|e| format!("transcript join: {e}"))??;

    if let Some(transcript) = &result {
        state
            .transcript_cache
            .lock()
            .await
            .insert(stem, transcript.clone());
    }
    Ok(result)
}

/// Clear the entire transcript cache. Called from `commands::index`
/// after a successful indexing run that wrote at least one whisper
/// sidecar. We chose full-clear over per-stem invalidation because
/// the indexer reports outcomes by `AssetId`, not stem, and the
/// id→stem mapping (proxy_path_for) requires a raw/ walk per
/// asset. Re-parsing one or two cached transcripts on the next
/// tab toggle is cheaper than that walk.
pub async fn clear_transcript_cache(state: &AwidatState) {
    state.transcript_cache.lock().await.clear();
}

/// Rename every occurrence of a speaker label in the whisper sidecar
/// for `stem`. Used by the transcript pane's inline speaker rename
/// (e.g. `SPEAKER_00` → `Host`). The sidecar is the canonical
/// store — no separate name-map sidecar — so the change is durable
/// across re-runs unless the user re-indexes whisper (which would
/// regenerate the auto-labels).
///
/// Rewrites three locations in the sidecar:
///   - `data.speakers[].id` where it matches `old_id`
///   - `data.segments[].speaker_id` where it matches `old_id`
///   - `data.words[].speaker_id` where it matches `old_id`
///
/// Returns the number of rewrites applied so the UI can confirm
/// or surface "no speaker matched". Errors when nothing matched —
/// indicates the user's `old_id` doesn't exist in the sidecar.
#[tauri::command]
pub async fn rename_speaker(
    state: State<'_, AwidatState>,
    stem: String,
    old_id: String,
    new_id: String,
) -> Result<usize, String> {
    let trimmed = new_id.trim();
    if trimmed.is_empty() {
        return Err("new speaker name cannot be empty".into());
    }
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let new_owned = trimmed.to_string();
    let stem_clone = stem.clone();
    let count = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let asset_abs = resolve_asset_for_stem(&project_root, &stem_clone)
            .ok_or_else(|| format!("no asset for stem {stem_clone}"))?;
        let asset_id_rel = asset_abs
            .strip_prefix(&project_root)
            .map_err(|_| format!("asset {} not under project root", asset_abs.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let asset_id = AssetId::new(asset_id_rel);
        // Reuse the canonical sidecar-path resolver from awidat-index so
        // the path layout (including the `raw/` segment and the file's
        // original extension) stays consistent with `read_transcript`.
        let sidecar_path = awidat_index::sidecar_path(&project_root, "whisper", &asset_id)
            .map_err(|e| format!("resolve whisper sidecar path: {e}"))?;
        let bytes = std::fs::read(&sidecar_path)
            .map_err(|e| format!("read whisper sidecar {}: {e}", sidecar_path.display()))?;
        let mut json: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("parse whisper sidecar: {e}"))?;
        let mut count = 0_usize;
        rewrite_speaker_id(&mut json, &old_id, &new_owned, &mut count);
        if count == 0 {
            return Err(format!("no speaker_id matched {old_id} in sidecar"));
        }
        let serialized = serde_json::to_vec_pretty(&json)
            .map_err(|e| format!("serialize whisper sidecar: {e}"))?;
        std::fs::write(&sidecar_path, serialized)
            .map_err(|e| format!("write whisper sidecar: {e}"))?;
        Ok(count)
    })
    .await
    .map_err(|e| format!("rename_speaker join: {e}"))??;

    // Drop any cached transcript so the next read sees the rename.
    state.transcript_cache.lock().await.remove(&stem);
    Ok(count)
}

/// Recursively rewrite every `id` (on speaker entries) and every
/// `speaker_id` (on segments + words) that equals `old`. Mutates in
/// place; increments `count` for each rewrite.
fn rewrite_speaker_id(value: &mut serde_json::Value, old: &str, new: &str, count: &mut usize) {
    match value {
        serde_json::Value::Object(map) => {
            // `speakers` array entries have `id`; segments and words
            // have `speaker_id`. Match either when the string equals
            // `old`. We don't disambiguate by parent key — a sidecar
            // never has a non-speaker `id == "SPEAKER_00"` field in
            // practice, so the false-positive risk is negligible.
            for (key, child) in map.iter_mut() {
                if (key == "id" || key == "speaker_id") && child.as_str() == Some(old) {
                    *child = serde_json::Value::String(new.to_string());
                    *count += 1;
                } else {
                    rewrite_speaker_id(child, old, new, count);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for child in arr.iter_mut() {
                rewrite_speaker_id(child, old, new, count);
            }
        }
        _ => {}
    }
}

/// Walk `<project>/raw/` and find the asset whose proxy filename
/// stem matches `stem`. The proxy stem is `<asset_stem>-<8hex>` —
/// recomputing the hash off the absolute asset path is the only
/// way to invert this mapping (we don't index proxy → asset on
/// disk anywhere else, intentionally; the proxy path is the
/// canonical handle).
fn resolve_asset_for_stem(project_root: &Path, stem: &str) -> Option<std::path::PathBuf> {
    let raw_dir = project_root.join("raw");
    if !raw_dir.is_dir() {
        return None;
    }
    let proxies_dir = project_root.join(".awidat").join("proxies");
    let target_proxy_name = format!("{stem}.mp4");
    walk_for_match(&raw_dir, &proxies_dir, &target_proxy_name)
}

fn walk_for_match(
    dir: &Path,
    proxies_dir: &Path,
    target_proxy_name: &str,
) -> Option<std::path::PathBuf> {
    // The incoming stem can come from two places:
    //   - a proxy filename (post-transcode, e.g. `foo-1080p-deadbeef`)
    //   - a raw asset filename (when no proxy exists yet, the frontend
    //     falls back to `<source-basename>` so the transcript pane can
    //     still resolve a clip whose whisper sidecar landed before the
    //     proxy did)
    // Strip the `.mp4` we appended in `read_transcript` so we can also
    // compare against raw filenames directly.
    let raw_stem_target = target_proxy_name
        .strip_suffix(".mp4")
        .unwrap_or(target_proxy_name);
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = walk_for_match(&path, proxies_dir, target_proxy_name) {
                return Some(found);
            }
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let proxy_path = proxy_path_for(proxies_dir, &path);
        if proxy_path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|n| n == target_proxy_name)
        {
            return Some(path);
        }
        // Fallback: match the raw filename (with or without extension)
        // so transcripts work for assets that have a whisper sidecar
        // but no proxy yet. Both `copy_F65206FA....MOV` and
        // `copy_F65206FA....` are accepted; the indexer keys its
        // sidecars on the source filename, so either reaches the right
        // sidecar.
        let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if file_name == raw_stem_target || file_stem == raw_stem_target {
            return Some(path);
        }
    }
    None
}

// --- Sidecar parsing ----------------------------------------------

#[derive(Debug, Deserialize)]
struct WhisperSidecarBody {
    #[serde(default)]
    language: String,
    #[serde(default)]
    diarized: bool,
    #[serde(default)]
    segments: Vec<RawSegment>,
    #[serde(default)]
    words: Vec<RawWord>,
    #[serde(default)]
    speakers: Vec<RawSpeaker>,
}

#[derive(Debug, Deserialize)]
struct RawSegment {
    text: String,
    start_s: f64,
    end_s: f64,
    #[serde(default)]
    speaker_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawWord {
    text: String,
    start_s: f64,
    end_s: f64,
    #[serde(default)]
    speaker_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawSpeaker {
    id: String,
    total_speech_s: f64,
}

fn parse_transcript(asset_stem: String, sidecar: serde_json::Value) -> Result<Transcript, String> {
    let body_value = sidecar
        .get("data")
        .cloned()
        .ok_or_else(|| "whisper sidecar missing `data` field".to_string())?;
    let body: WhisperSidecarBody = serde_json::from_value(body_value)
        .map_err(|e| format!("whisper sidecar `data` malformed: {e}"))?;
    let mut segments: Vec<TranscriptSegment> = body
        .segments
        .into_iter()
        .map(|s| TranscriptSegment {
            text: s.text,
            start_s: s.start_s,
            end_s: s.end_s,
            speaker_id: s.speaker_id,
        })
        .collect();
    let mut words: Vec<TranscriptWord> = body
        .words
        .into_iter()
        .map(|w| TranscriptWord {
            text: w.text,
            start_s: w.start_s,
            end_s: w.end_s,
            speaker_id: w.speaker_id,
        })
        .collect();
    // Sort defensively — the indexer should produce sorted output
    // but a future schema migration shouldn't silently break the
    // virtualized transcript pane (which assumes ordered segments).
    segments.sort_by(|a, b| {
        a.start_s
            .partial_cmp(&b.start_s)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    words.sort_by(|a, b| {
        a.start_s
            .partial_cmp(&b.start_s)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let speakers = body
        .speakers
        .into_iter()
        .map(|s| TranscriptSpeaker {
            id: s.id,
            total_speech_s: s.total_speech_s,
        })
        .collect();
    Ok(Transcript {
        asset_stem,
        language: body.language,
        diarized: body.diarized,
        segments,
        words,
        speakers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_transcript_extracts_data_body() {
        let sidecar = serde_json::json!({
            "indexer": "whisper",
            "indexer_version": "0.1.0",
            "schema_version": "1.0",
            "asset_id": "raw/foo.mov",
            "asset_sha256": "abc",
            "produced_at": "2025-01-01T00:00:00Z",
            "data": {
                "language": "en",
                "model": "tiny",
                "diarized": true,
                "segments": [
                    {"text": "Hello world", "start_s": 0.0, "end_s": 1.5, "speaker_id": "SPEAKER_00"},
                    {"text": "Foo bar", "start_s": 1.5, "end_s": 3.0, "speaker_id": "SPEAKER_01"},
                ],
                "words": [
                    {"text": "Hello", "start_s": 0.0, "end_s": 0.7},
                    {"text": "world", "start_s": 0.7, "end_s": 1.5},
                ],
                "speakers": [
                    {"id": "SPEAKER_00", "total_speech_s": 1.5},
                    {"id": "SPEAKER_01", "total_speech_s": 1.5},
                ]
            }
        });
        let t = parse_transcript("foo-12345678".into(), sidecar).unwrap();
        assert_eq!(t.asset_stem, "foo-12345678");
        assert_eq!(t.language, "en");
        assert!(t.diarized);
        assert_eq!(t.segments.len(), 2);
        assert_eq!(t.words.len(), 2);
        assert_eq!(t.speakers.len(), 2);
        assert_eq!(t.segments[0].speaker_id.as_deref(), Some("SPEAKER_00"));
    }

    #[test]
    fn parse_transcript_sorts_segments_by_start() {
        let sidecar = serde_json::json!({
            "indexer": "whisper",
            "indexer_version": "0.1.0",
            "schema_version": "1.0",
            "asset_id": "raw/foo.mov",
            "asset_sha256": "abc",
            "produced_at": "2025-01-01T00:00:00Z",
            "data": {
                "language": "en",
                "model": "tiny",
                "diarized": false,
                "segments": [
                    {"text": "second", "start_s": 5.0, "end_s": 6.0},
                    {"text": "first",  "start_s": 0.0, "end_s": 1.0},
                ],
                "words": [],
                "speakers": []
            }
        });
        let t = parse_transcript("foo".into(), sidecar).unwrap();
        assert_eq!(t.segments[0].text, "first");
        assert_eq!(t.segments[1].text, "second");
    }

    #[test]
    fn parse_transcript_rejects_missing_data() {
        let sidecar = serde_json::json!({
            "indexer": "whisper",
        });
        let err = parse_transcript("x".into(), sidecar).unwrap_err();
        assert!(err.contains("missing `data`"), "got: {err}");
    }

    #[test]
    fn walk_for_match_resolves_by_proxy_stem_when_proxy_exists() {
        // Normal case: caller hands us a proxy-stem-shaped string and
        // we recompute the expected proxy filename from each raw asset
        // to find the matching one.
        let dir = tempfile::tempdir().unwrap();
        let raw_dir = dir.path().join("raw");
        let proxies_dir = dir.path().join(".awidat").join("proxies");
        std::fs::create_dir_all(&raw_dir).unwrap();
        std::fs::create_dir_all(&proxies_dir).unwrap();
        let asset = raw_dir.join("clip.MOV");
        std::fs::write(&asset, b"src").unwrap();
        let expected_proxy_name = proxy_path_for(&proxies_dir, &asset)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let found = walk_for_match(&raw_dir, &proxies_dir, &expected_proxy_name).unwrap();
        assert_eq!(found, asset);
    }

    #[test]
    fn walk_for_match_resolves_by_raw_filename_when_no_proxy_exists() {
        // The bug-fix path: whisper sidecar landed but no proxy yet.
        // Frontend passes `<source-basename>` (with or without
        // extension), backend has to resolve it via raw/ filename
        // match rather than the proxy-stem reverse lookup.
        let dir = tempfile::tempdir().unwrap();
        let raw_dir = dir.path().join("raw");
        let proxies_dir = dir.path().join(".awidat").join("proxies");
        std::fs::create_dir_all(&raw_dir).unwrap();
        std::fs::create_dir_all(&proxies_dir).unwrap();
        let asset = raw_dir.join("clip.MOV");
        std::fs::write(&asset, b"src").unwrap();
        // With extension — the `.mp4` suffix added by read_transcript
        // gets stripped before the raw-filename comparison, so the
        // target is e.g. "clip.MOV".
        let target_with_ext = "clip.MOV.mp4";
        let found = walk_for_match(&raw_dir, &proxies_dir, target_with_ext).unwrap();
        assert_eq!(found, asset);
        // Without extension.
        let target_no_ext = "clip.mp4";
        let found = walk_for_match(&raw_dir, &proxies_dir, target_no_ext).unwrap();
        assert_eq!(found, asset);
    }

    #[test]
    fn walk_for_match_returns_none_when_nothing_matches() {
        let dir = tempfile::tempdir().unwrap();
        let raw_dir = dir.path().join("raw");
        let proxies_dir = dir.path().join(".awidat").join("proxies");
        std::fs::create_dir_all(&raw_dir).unwrap();
        std::fs::create_dir_all(&proxies_dir).unwrap();
        std::fs::write(raw_dir.join("clip.MOV"), b"src").unwrap();
        assert!(walk_for_match(&raw_dir, &proxies_dir, "unrelated.mp4").is_none());
    }
}
