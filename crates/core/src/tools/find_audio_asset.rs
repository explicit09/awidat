//! `find_audio_asset` tool — return ranked matches from the bundled
//! audio starter pack.
//!
//! Background: beat plumbing (find_beat / beats-mcp) is wired up, but
//! the agent has no library or affordance to actually propose "drop a
//! whoosh on that beat." This tool ships the *system* — a metadata
//! schema, a tiny synthetic CC0 starter pack at
//! `<workspace>/assets/audio/`, and a typed read-only lookup.
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
//! `path` in the index is *relative* to the pack root. The tool
//! resolves it to an absolute path before returning, so the agent
//! can hand the result straight to ffmpeg / apply_edl.
//!
//! Pack discovery order (first hit wins):
//!   1. `AWIDAT_AUDIO_PACK_ROOT` env var — for tests and packaging.
//!   2. `<workspace>/assets/audio` baked from `CARGO_MANIFEST_DIR`
//!      at compile time — covers `cargo run` / `cargo test`.
//!   3. `<exe-dir>/assets/audio` — co-located install layout.
//!
//! Ranking: matches with more overlapping mood tags rank higher.
//! Within the same overlap count, shorter duration wins (typical
//! "drop a quick SFX here" intent). Ties broken by slug.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Hard cap on results returned to the agent. The pack is small and
/// the agent rarely needs more than the top few candidates.
const DEFAULT_MAX_RESULTS: usize = 8;
const HARD_MAX_RESULTS: usize = 32;

/// Bundled index — embedded only for the in-crate parse-sanity test
/// (`bundled_index_parses_cleanly`). The shipped tool reads
/// `<pack_root>/index.json` on demand so packagers can swap in a
/// curated pack without recompiling.
#[cfg(test)]
const BUNDLED_INDEX_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/audio/index.json"
));

/// The `find_audio_asset` tool.
pub struct FindAudioAssetTool;

#[derive(Debug, Deserialize)]
struct FindAudioAssetArgs {
    /// `sfx` | `music` | `ambience`.
    kind: String,
    /// Free-text mood tag — matched against entry `mood` tags
    /// case-insensitively. Optional; when omitted, all entries of
    /// `kind` are returned ordered by duration ascending.
    #[serde(default)]
    mood: Option<String>,
    /// Drop entries longer than this many seconds. Optional.
    #[serde(default)]
    max_duration_s: Option<f32>,
    /// Cap on returned results. Default 8, hard cap 32.
    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl ToolHandler for FindAudioAssetTool {
    fn name(&self) -> &'static str {
        "find_audio_asset"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_audio_asset".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["sfx", "music", "ambience"],
                        "description": "Category to filter by."
                    },
                    "mood": {
                        "type": "string",
                        "description": "Free-text mood tag (e.g. 'hype', 'tension', 'calm'). Optional."
                    },
                    "max_duration_s": {
                        "type": "number",
                        "minimum": 0.0,
                        "description": "Drop entries longer than this many seconds. Optional."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 32,
                        "description": "Cap on returned results. Default 8."
                    }
                },
                "required": ["kind"]
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
        _ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: FindAudioAssetArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_audio_asset: invalid args ({e}). Required: kind ∈ \
                 {{sfx, music, ambience}}. Optional: mood, max_duration_s, \
                 max_results."
            ))
        })?;

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
                    "note": "No audio pack found. Set AWIDAT_AUDIO_PACK_ROOT or ship assets/audio/."
                });
                return Ok(ToolOutput::text(body.to_string()));
            }
        };

        let mut results = find_audio_assets(
            &pack_root,
            &args.kind,
            args.mood.as_deref(),
            args.max_duration_s,
        )
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_audio_asset: failed to load audio pack at \
                         {}: {e}",
                pack_root.display()
            ))
        })?;

        let more_available = results.len() > max_results;
        results.truncate(max_results);

        let body = serde_json::json!({
            "pack_root": pack_root,
            "results": results,
            "more_available": more_available,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
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
    let index: PackIndex = serde_json::from_slice(&bytes)
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
    if let Ok(env_root) = std::env::var("AWIDAT_AUDIO_PACK_ROOT") {
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

const DESCRIPTION: &str = "\
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_index_parses_cleanly() {
        // Sanity-check the build-time `include_str!`: if `index.json`
        // drifts from the schema, the workspace won't compile, and
        // this test reads it once more at runtime to make sure
        // duration_s + slug + path round-trip.
        let parsed: PackIndex = serde_json::from_str(BUNDLED_INDEX_JSON)
            .unwrap_or_else(|e| panic!("bundled index.json must parse: {e}"));
        assert_eq!(parsed.version, 1);
        assert!(
            !parsed.entries.is_empty(),
            "bundled index should ship at least one entry"
        );
        for e in &parsed.entries {
            assert!(!e.slug.is_empty());
            assert!(!e.path.is_empty(), "{} missing path", e.slug);
            assert!(
                matches!(e.kind.as_str(), "sfx" | "music" | "ambience"),
                "{} has unexpected kind {}",
                e.slug,
                e.kind
            );
            assert!(
                e.duration_s > 0.0,
                "{} has non-positive duration {}",
                e.slug,
                e.duration_s
            );
        }
    }

    #[test]
    fn rank_prefers_mood_match_over_duration() {
        let entries = vec![
            PackEntry {
                slug: "short_neutral".into(),
                path: "sfx/short_neutral.wav".into(),
                kind: "sfx".into(),
                mood: vec!["ui".into()],
                duration_s: 0.05,
                license: "CC0-1.0".into(),
            },
            PackEntry {
                slug: "long_hype".into(),
                path: "sfx/long_hype.wav".into(),
                kind: "sfx".into(),
                mood: vec!["hype".into()],
                duration_s: 1.0,
                license: "CC0-1.0".into(),
            },
        ];
        let out = filter_and_rank(entries, Path::new("/pack"), "sfx", Some("hype"), None);
        // Hype match must come first even though it's longer.
        assert_eq!(out.len(), 1, "non-matching mood must be filtered out");
        assert_eq!(out[0].slug, "long_hype");
    }

    #[test]
    fn no_mood_returns_all_kind_matches_ordered_by_duration() {
        let entries = vec![
            PackEntry {
                slug: "med".into(),
                path: "sfx/med.wav".into(),
                kind: "sfx".into(),
                mood: vec![],
                duration_s: 0.5,
                license: "CC0-1.0".into(),
            },
            PackEntry {
                slug: "short".into(),
                path: "sfx/short.wav".into(),
                kind: "sfx".into(),
                mood: vec![],
                duration_s: 0.1,
                license: "CC0-1.0".into(),
            },
            PackEntry {
                slug: "music_only".into(),
                path: "music/m.wav".into(),
                kind: "music".into(),
                mood: vec![],
                duration_s: 0.2,
                license: "CC0-1.0".into(),
            },
        ];
        let out = filter_and_rank(entries, Path::new("/pack"), "sfx", None, None);
        assert_eq!(out.len(), 2, "music entry must be filtered out");
        assert_eq!(out[0].slug, "short");
        assert_eq!(out[1].slug, "med");
    }

    #[test]
    fn max_duration_filters_long_entries() {
        let entries = vec![
            PackEntry {
                slug: "short".into(),
                path: "sfx/s.wav".into(),
                kind: "sfx".into(),
                mood: vec![],
                duration_s: 0.2,
                license: "CC0-1.0".into(),
            },
            PackEntry {
                slug: "long".into(),
                path: "sfx/l.wav".into(),
                kind: "sfx".into(),
                mood: vec![],
                duration_s: 5.0,
                license: "CC0-1.0".into(),
            },
        ];
        let out = filter_and_rank(entries, Path::new("/pack"), "sfx", None, Some(1.0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].slug, "short");
    }

    #[test]
    fn resolves_relative_index_path_to_absolute() {
        let entries = vec![PackEntry {
            slug: "x".into(),
            path: "sfx/x.wav".into(),
            kind: "sfx".into(),
            mood: vec![],
            duration_s: 0.1,
            license: "CC0-1.0".into(),
        }];
        let pack = Path::new("/abs/pack");
        let out = filter_and_rank(entries, pack, "sfx", None, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, pack.join("sfx/x.wav"));
    }
}
