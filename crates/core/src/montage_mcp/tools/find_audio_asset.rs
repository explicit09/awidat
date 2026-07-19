//! `find_audio_asset` — ranked matches from the bundled audio starter
//! pack. Ported from `crates/core/src/tools/find_audio_asset.rs` to
//! the in-process MCP server.
//!
//! Library convention (see `assets/audio/index.json`):
//!   {
//!     "version": 1,
//!     "entries": [
//!       { "slug": "...", "path": "sfx/whoosh.wav",
//!         "kind": "sfx" | "music" | "ambience",
//!         "mood": ["hype", "transition", ...],
//!         "duration_s": 0.6, "license": "CC0-1.0" }
//!     ]
//!   }
//!
//! Pack discovery order (first hit wins):
//!   1. `MONTAGE_AUDIO_PACK_ROOT` env var — for tests and packaging.
//!   2. `<workspace>/assets/audio` baked from `CARGO_MANIFEST_DIR`
//!      at compile time — covers `cargo run` / `cargo test`.
//!   3. `<exe-dir>/assets/audio` — co-located install layout.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Hard cap on results returned to the agent. The pack is small and
/// the agent rarely needs more than the top few candidates.
const DEFAULT_MAX_RESULTS: usize = 8;
const HARD_MAX_RESULTS: usize = 32;

/// Arguments to `find_audio_asset`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct FindAudioAssetArgs {
    /// `sfx` | `music` | `ambience`.
    pub kind: String,
    /// Free-text mood tag — matched against entry `mood` tags
    /// case-insensitively. Optional; when omitted, all entries of
    /// `kind` are returned ordered by duration ascending.
    #[serde(default)]
    pub mood: Option<String>,
    /// Drop entries longer than this many seconds. Optional.
    #[serde(default)]
    pub max_duration_s: Option<f32>,
    /// Cap on returned results. Default 8, hard cap 32.
    #[serde(default)]
    pub max_results: Option<usize>,
}

/// Run `find_audio_asset` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`; pack-load
/// errors return `Err(String)`. An absent pack returns an empty
/// result set (not an error).
pub fn run(args: FindAudioAssetArgs, _ctx: McpToolCtx) -> Result<String, String> {
    let max_results = args
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .min(HARD_MAX_RESULTS);

    let pack_root = match resolve_pack_root() {
        Some(root) => root,
        None => {
            // No pack on disk — return a structured empty result
            // so the agent surfaces it as "no library yet" instead
            // of as an opaque error.
            let body = serde_json::json!({
                "pack_root": null,
                "results": [],
                "more_available": false,
                "note": "No audio pack found. Set MONTAGE_AUDIO_PACK_ROOT or ship assets/audio/."
            });
            return Ok(body.to_string());
        }
    };

    let mut results = find_audio_assets(
        &pack_root,
        &args.kind,
        args.mood.as_deref(),
        args.max_duration_s,
    )
    .map_err(|e| {
        format!(
            "find_audio_asset: failed to load audio pack at {}: {e}",
            pack_root.display()
        )
    })?;

    let more_available = results.len() > max_results;
    results.truncate(max_results);

    let body = serde_json::json!({
        "pack_root": pack_root,
        "results": results,
        "more_available": more_available,
    });
    Ok(body.to_string())
}

/// One ranked match returned by the lookup. Serialized into the
/// tool's JSON output.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AudioAssetMatch {
    /// Stable identifier (e.g. "whoosh_hype").
    pub slug: String,
    /// Absolute path to the audio file on disk.
    pub path: PathBuf,
    /// `sfx` | `music` | `ambience`.
    pub kind: String,
    /// Mood tags from the index. Useful to the agent when picking
    /// between near-ties.
    pub mood: Vec<String>,
    /// Duration in seconds, copied from the index.
    pub duration_s: f64,
    /// License string (e.g. "CC0-1.0").
    pub license: String,
}

/// Mirror of one entry in `assets/audio/index.json`. Private — callers
/// receive the resolved [`AudioAssetMatch`] which has absolute paths.
#[derive(Debug, Clone, serde::Deserialize)]
struct PackEntry {
    slug: String,
    path: String,
    kind: String,
    #[serde(default)]
    mood: Vec<String>,
    duration_s: f64,
    #[serde(default = "default_license")]
    license: String,
}

fn default_license() -> String {
    "CC0-1.0".into()
}

#[derive(Debug, Clone, serde::Deserialize)]
struct PackIndex {
    #[serde(default = "default_version")]
    version: u32,
    entries: Vec<PackEntry>,
}

fn default_version() -> u32 {
    1
}

/// Pure lookup over the audio pack. Returns matches ordered by
/// mood-tag overlap (desc), then duration (asc), then slug (asc).
///
/// Loads `<pack_root>/index.json` lazily. Errors out only on
/// I/O / parse failures — empty match sets are not errors.
pub fn find_audio_assets(
    pack_root: &Path,
    kind: &str,
    mood: Option<&str>,
    max_duration_s: Option<f32>,
) -> Result<Vec<AudioAssetMatch>, String> {
    let index_path = pack_root.join("index.json");
    let bytes = std::fs::read(&index_path).map_err(|e| {
        format!(
            "failed to read audio pack index at {}: {e}",
            index_path.display()
        )
    })?;
    let index: PackIndex = montage_proto::serde_robust::from_json_slice(&bytes)
        .map_err(|e| format!("failed to parse {}: {e}", index_path.display()))?;
    if index.version != 1 {
        return Err(format!(
            "unsupported audio pack version {}; expected 1",
            index.version
        ));
    }
    Ok(filter_and_rank(
        index.entries,
        pack_root,
        kind,
        mood,
        max_duration_s,
    ))
}

/// Pure ranking logic — extracted so it can be unit-tested without
/// touching the filesystem.
fn filter_and_rank(
    entries: Vec<PackEntry>,
    pack_root: &Path,
    kind: &str,
    mood: Option<&str>,
    max_duration_s: Option<f32>,
) -> Vec<AudioAssetMatch> {
    let kind_lower = kind.to_ascii_lowercase();
    let mood_lower = mood.map(|m| m.to_ascii_lowercase());

    let mut scored: Vec<(u32, AudioAssetMatch)> = entries
        .into_iter()
        .filter(|e| e.kind.eq_ignore_ascii_case(&kind_lower))
        .filter(|e| match max_duration_s {
            Some(cap) => e.duration_s <= f64::from(cap) + 1e-9,
            None => true,
        })
        .filter_map(|e| {
            let overlap = match &mood_lower {
                Some(needle) => {
                    let direct: u32 = e
                        .mood
                        .iter()
                        .filter(|m| m.eq_ignore_ascii_case(needle))
                        .count() as u32;
                    let substring: u32 = e
                        .mood
                        .iter()
                        .filter(|m| {
                            !m.eq_ignore_ascii_case(needle)
                                && m.to_ascii_lowercase().contains(needle)
                        })
                        .count() as u32;
                    // Direct matches dominate; substring matches still
                    // count as evidence (e.g. mood="whoosh" matches an
                    // entry tagged "whoosh-soft").
                    let total = direct.saturating_mul(2).saturating_add(substring);
                    if total == 0 {
                        // Caller asked for a specific mood and this
                        // entry doesn't have it at all — skip.
                        return None;
                    }
                    total
                }
                None => 0,
            };

            let resolved_path = pack_root.join(&e.path);
            Some((
                overlap,
                AudioAssetMatch {
                    slug: e.slug,
                    path: resolved_path,
                    kind: e.kind,
                    mood: e.mood,
                    duration_s: e.duration_s,
                    license: e.license,
                },
            ))
        })
        .collect();

    // Sort: overlap desc, duration asc, slug asc.
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| {
                a.1.duration_s
                    .partial_cmp(&b.1.duration_s)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.1.slug.cmp(&b.1.slug))
    });

    scored.into_iter().map(|(_, m)| m).collect()
}

/// Probe for the audio pack root. Returns `None` if no candidate
/// exists on disk — callers surface that as an empty result, not an
/// error (so a fresh checkout without the pack just gets an empty
/// list instead of a tool crash).
fn resolve_pack_root() -> Option<PathBuf> {
    // 1. Explicit override — tests + packaged installs.
    if let Ok(env_root) = std::env::var("MONTAGE_AUDIO_PACK_ROOT") {
        let p = PathBuf::from(env_root);
        if p.join("index.json").is_file() {
            return Some(p);
        }
    }

    // 2. Compile-time workspace root (cargo run / cargo test).
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(workspace) = manifest_dir.parent().and_then(|p| p.parent()) {
        let candidate = workspace.join("assets").join("audio");
        if candidate.join("index.json").is_file() {
            return Some(candidate);
        }
    }

    // 3. Co-located install layout: <exe-dir>/assets/audio.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let candidate = exe_dir.join("assets").join("audio");
            if candidate.join("index.json").is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

pub const DESCRIPTION: &str = "\
Search the bundled audio starter pack for a sound to drop on the \
timeline. Returns a ranked list of matching audio files (sfx, music, \
or ambience) with absolute paths suitable for apply_edl / ffmpeg.\
\n\nUse this when the user says 'drop a whoosh on that beat', \
'find a tense riser for the hook', or 'add ambient hum under the \
intro'. Pair with find_beat to anchor SFX to musical/spoken beats.\
\n\nArgs: kind (required, one of sfx/music/ambience), mood (optional \
free-text tag like 'hype' or 'tension'), max_duration_s (optional \
upper bound), max_results (default 8, hard cap 32).\
\n\nResults are ranked by mood-tag overlap first, then by duration \
ascending. When the pack is empty or absent, returns an empty results \
list (not an error) — surface that as 'no SFX library available' \
rather than asserting the request failed.\
\n\nThe starter pack ships under CC0-1.0 and contains a handful of \
synthetic effects (whoosh, riser, impact, ambient hum, ui click). \
Treat it as a smoke-test library — real curation belongs in a \
follow-up.\
";
