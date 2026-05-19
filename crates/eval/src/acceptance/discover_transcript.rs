//! Transcript-sidecar discovery for real-video acceptance fixture authoring.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use awidat_core::transcript_cleanup::{
    CleanupConfig, TranscriptSegment, TranscriptWord, false_start_ranges, filler_dense_ranges,
    normalize_transcript_token,
};
use serde::Serialize;
use serde_json::Value;

/// One transcript-backed behavior candidate from an existing Whisper sidecar.
#[derive(Debug, Serialize)]
pub struct TranscriptDiscoveryCandidate {
    /// Absolute sidecar path that supplied the evidence.
    pub sidecar_path: String,
    /// Source asset id recorded in the sidecar.
    pub asset_id: String,
    /// Resolved absolute source media path when the sidecar sits under an Awidat project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    /// Suggested behavior type for a future acceptance fixture.
    pub suggested_behavior: String,
    /// Source start in sidecar seconds.
    pub start_s: f64,
    /// Source end in sidecar seconds.
    pub end_s: f64,
    /// Optional marker that triggered the candidate.
    pub marker: Option<String>,
    /// Transcript snippet for human review.
    pub snippet: String,
    /// Heuristic authoring priority.
    pub score: f64,
    /// Fixture id starter derived from the asset id.
    pub fixture_id_hint: String,
}

/// Discover transcript-backed acceptance candidates from Whisper sidecars.
pub(crate) fn discover_transcript_candidates(
    root: &Path,
    max_depth: usize,
    max_files: usize,
) -> Result<Vec<TranscriptDiscoveryCandidate>> {
    let mut sidecars = collect_whisper_sidecars(root, max_depth)?;
    sidecars.truncate(max_files);

    let mut candidates = Vec::new();
    for path in sidecars {
        let sidecar = read_sidecar(&path)?;
        candidates.extend(false_start_candidates(&path, &sidecar));
        candidates.extend(filler_candidates(&path, &sidecar));
    }
    candidates.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.sidecar_path.cmp(&right.sidecar_path))
            .then_with(|| left.start_s.total_cmp(&right.start_s))
    });
    Ok(candidates)
}

fn collect_whisper_sidecars(root: &Path, max_depth: usize) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    collect_whisper_sidecars_inner(root, max_depth, 0, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_whisper_sidecars_inner(
    dir: &Path,
    max_depth: usize,
    depth: usize,
    paths: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        if is_hidden_path(&path) {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            if depth < max_depth {
                collect_whisper_sidecars_inner(&path, max_depth, depth + 1, paths)?;
            }
        } else if file_type.is_file() && is_whisper_sidecar_path(&path) {
            paths.push(path);
        }
    }
    Ok(())
}

fn read_sidecar(path: &Path) -> Result<TranscriptSidecar> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let value: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    let asset_id = value
        .get("asset_id")
        .and_then(|asset| asset.as_str())
        .unwrap_or("unknown")
        .to_string();
    let words = value
        .pointer("/data/words")
        .and_then(|words| words.as_array())
        .into_iter()
        .flatten()
        .filter_map(transcript_word_from_json)
        .collect::<Vec<_>>();
    let segments = value
        .pointer("/data/segments")
        .and_then(|segments| segments.as_array())
        .into_iter()
        .flatten()
        .filter_map(transcript_segment_from_json)
        .collect::<Vec<_>>();
    Ok(TranscriptSidecar {
        source_path: resolve_source_path(path, &asset_id),
        asset_id,
        words,
        segments,
    })
}

fn false_start_candidates(
    path: &Path,
    sidecar: &TranscriptSidecar,
) -> Vec<TranscriptDiscoveryCandidate> {
    let Some(source_end_s) = sidecar.words.iter().map(|word| word.end_s).reduce(f64::max) else {
        return Vec::new();
    };
    false_start_ranges(&sidecar.words, 0.0, source_end_s)
        .into_iter()
        .filter(|range| is_discovery_false_start_marker(&range.marker))
        .map(|range| TranscriptDiscoveryCandidate {
            sidecar_path: path.to_string_lossy().into_owned(),
            asset_id: sidecar.asset_id.clone(),
            source_path: sidecar.source_path.clone(),
            suggested_behavior: "false_start_cleanup".into(),
            start_s: range.start_s,
            end_s: range.end_s,
            marker: Some(range.marker),
            snippet: range.snippet,
            score: 80.0,
            fixture_id_hint: fixture_id_hint(&sidecar.asset_id, "false_start"),
        })
        .collect()
}

fn is_discovery_false_start_marker(marker: &str) -> bool {
    let normalized = normalize_transcript_token(marker);
    if normalized != "actually" {
        return true;
    }
    marker.trim().ends_with([',', '?', '!'])
}

fn filler_candidates(
    path: &Path,
    sidecar: &TranscriptSidecar,
) -> Vec<TranscriptDiscoveryCandidate> {
    filler_dense_ranges(&sidecar.segments, CleanupConfig::default())
        .into_iter()
        .map(|range| TranscriptDiscoveryCandidate {
            sidecar_path: path.to_string_lossy().into_owned(),
            asset_id: sidecar.asset_id.clone(),
            source_path: sidecar.source_path.clone(),
            suggested_behavior: "transcript_filler_cleanup".into(),
            start_s: range.start_s,
            end_s: range.end_s,
            marker: None,
            snippet: sidecar
                .segments
                .iter()
                .find(|segment| segment.start_s == range.start_s && segment.end_s == range.end_s)
                .map(|segment| segment.text.clone())
                .unwrap_or_default(),
            score: 50.0 + range.filler_token_count as f64,
            fixture_id_hint: fixture_id_hint(&sidecar.asset_id, "filler_cleanup"),
        })
        .collect()
}

fn transcript_word_from_json(value: &Value) -> Option<TranscriptWord> {
    let text = value.get("text")?.as_str()?.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let start_s = value
        .get("start_s")
        .or_else(|| value.get("start"))?
        .as_f64()?;
    let end_s = value.get("end_s").or_else(|| value.get("end"))?.as_f64()?;
    (end_s > start_s).then_some(TranscriptWord {
        text,
        start_s,
        end_s,
    })
}

fn resolve_source_path(sidecar_path: &Path, asset_id: &str) -> Option<String> {
    if asset_id == "unknown" || Path::new(asset_id).is_absolute() {
        return None;
    }
    for ancestor in sidecar_path.parent()?.ancestors() {
        let candidate = ancestor.join(asset_id);
        if candidate.is_file() {
            let resolved = candidate.canonicalize().unwrap_or(candidate);
            return Some(resolved.to_string_lossy().into_owned());
        }
    }
    None
}

fn transcript_segment_from_json(value: &Value) -> Option<TranscriptSegment> {
    let text = value.get("text")?.as_str()?.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let start_s = value
        .get("start_s")
        .or_else(|| value.get("start"))?
        .as_f64()?;
    let end_s = value.get("end_s").or_else(|| value.get("end"))?.as_f64()?;
    (end_s > start_s).then_some(TranscriptSegment {
        start_s,
        end_s,
        text,
    })
}

fn is_whisper_sidecar_path(path: &Path) -> bool {
    if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
        return false;
    }
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .windows(2)
        .any(|window| window[0] == "index" && window[1] == "whisper")
}

fn is_hidden_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn fixture_id_hint(asset_id: &str, behavior: &str) -> String {
    let stem = Path::new(asset_id)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("transcript");
    let sanitized = stem
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    format!("acceptance::candidate_{sanitized}_{behavior}")
}

struct TranscriptSidecar {
    asset_id: String,
    source_path: Option<String>,
    words: Vec<TranscriptWord>,
    segments: Vec<TranscriptSegment>,
}
