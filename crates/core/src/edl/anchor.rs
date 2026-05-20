//! Resolve an [`Anchor`] to a clip inside an OTIO [`Timeline`].
//!
//! Mirrors the 4-tier cascade in
//! `harnesses/codex/codex-rs/apply-patch/src/seek_sequence.rs`, adapted
//! for video:
//! 1. **Exact substring** match against the clip's
//!    `metadata.awidat.anchor.transcript_snippet` (or marker text).
//! 2. **Trim-end** tolerance — also strip trailing whitespace on both sides.
//! 3. **Trim-both** tolerance — strip leading + trailing whitespace.
//! 4. **Unicode-normalize** — fold smart quotes, dashes, NBSPs to ASCII.
//! 5. **Clip-uuid fallback** — only if the model passed
//!    `Anchor::ClipUuid`, match exactly against `name` or
//!    `awidat.clip_uuid` extra. As a final recovery path, also match
//!    the clip's media-reference target URL / filename / stem when
//!    exactly one clip references that asset.
//!
//! On miss, return up to 3 closest candidates by simple character-overlap
//! score (Aider's "did you mean?" pattern at
//! `aider/aider/coders/editblock_coder.py:91-106`). The agent gets these
//! in the `RespondToModel` error so it can self-correct in the same turn.

use std::path::{Path, PathBuf};

use awidat_proto::otio::{StackChild, Timeline, TrackChild};

use super::op::Anchor;

/// Side data the resolver needs beyond the timeline itself. Bundles
/// the project root (so the resolver can read whisper sidecars to
/// match transcript phrases that aren't pre-seeded as clip metadata)
/// and a cache of already-loaded sidecars to avoid hitting disk
/// repeatedly inside one apply pass.
///
/// `AnchorContext::empty()` returns a context with no project root —
/// the resolver falls back to clip-metadata-only matching, which is
/// what every existing test relies on.
#[derive(Debug, Default)]
pub struct AnchorContext {
    project_root: Option<PathBuf>,
    /// Cache: asset_id → segments[]. Each segment is `(start_s,
    /// end_s, text_lower)` ordered by start_s. Built lazily.
    sidecar_cache: std::cell::RefCell<std::collections::HashMap<String, Vec<TranscriptSegment>>>,
}

#[derive(Debug, Clone)]
struct TranscriptSegment {
    start_s: f64,
    end_s: f64,
    text: String,
}

impl AnchorContext {
    /// No project root — resolver matches against clip metadata only.
    /// This is the right context for most unit tests.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Resolver may read whisper sidecars under `<project_root>/index/whisper/`
    /// when matching transcript snippets that aren't seeded as clip
    /// metadata.
    pub fn with_project_root(root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: Some(root.into()),
            sidecar_cache: std::cell::RefCell::new(Default::default()),
        }
    }

    /// Project root if the context was constructed with one. Callers
    /// (e.g., apply-time LUT validation) use this to resolve
    /// project-relative paths against disk.
    pub fn project_root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }

    /// Look up segments for the asset, loading the sidecar from disk
    /// on first miss. Returns an empty Vec if the sidecar doesn't
    /// exist or can't be parsed — that's a soft miss; the resolver
    /// falls through to clip-metadata matching.
    fn segments_for(&self, asset_id: &str) -> Vec<TranscriptSegment> {
        let Some(root) = &self.project_root else {
            return Vec::new();
        };
        if let Some(hit) = self.sidecar_cache.borrow().get(asset_id) {
            return hit.clone();
        }
        let segments = load_segments(root, asset_id).unwrap_or_default();
        self.sidecar_cache
            .borrow_mut()
            .insert(asset_id.to_string(), segments.clone());
        segments
    }
}

fn load_segments(project_root: &Path, asset_id: &str) -> Option<Vec<TranscriptSegment>> {
    // Sidecar layout: <root>/index/whisper/<asset_id>.json. The asset
    // id already includes any `raw/` prefix.
    let path = project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset_id}.json"));
    let bytes = std::fs::read(&path).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let segs = v.pointer("/data/segments")?.as_array()?;
    let mut out: Vec<TranscriptSegment> = segs
        .iter()
        .filter_map(|s| {
            let text = s.get("text")?.as_str()?.to_string();
            let start_s = s.get("start_s").and_then(|v| v.as_f64())?;
            let end_s = s.get("end_s").and_then(|v| v.as_f64())?;
            Some(TranscriptSegment {
                start_s,
                end_s,
                text,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.start_s
            .partial_cmp(&b.start_s)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Some(out)
}

/// Where in the timeline a resolved clip lives. Track-relative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipLocator {
    /// Index into `timeline.tracks.children` (the Stack at root).
    pub track_index: usize,
    /// Index into the matched track's `children` vector.
    pub child_index: usize,
}

/// Resolve an anchor to a single clip locator. Returns `Err` with a
/// "did you mean?" candidates list on miss.
///
/// Resolution order for `transcript_snippet` anchors:
///
/// 1. Match the 4-tier cascade against each clip's seeded metadata
///    (transcript_snippet, marker notes, clip name).
/// 2. If `ctx.project_root` is set and tier 1 missed: load the whisper
///    sidecar for each clip's asset, run the same 4-tier cascade
///    against segment text, and map the matching segment back to a
///    clip whose source_range covers that segment timestamp.
///
/// Step 2 is what makes anchoring against real footage work — when
/// `awidat init` produces a clip with only `asset_id` in its anchor,
/// the agent can still say `transcript_snippet="builders network"`
/// and the resolver pulls the phrase out of the whisper transcript.
pub fn resolve(
    timeline: &Timeline,
    anchor: &Anchor,
    ctx: &AnchorContext,
) -> Result<ClipLocator, AnchorMiss> {
    let clips = collect_clips(timeline);

    let needle = match anchor {
        Anchor::TranscriptSnippet { text } => text.as_str(),
        Anchor::ClipUuid { uuid } => return resolve_by_uuid(&clips, uuid),
        Anchor::SceneChangeIndex { .. } => {
            return Err(AnchorMiss {
                anchor: anchor.clone(),
                candidates: Vec::new(),
                reason: "scene_change_index anchors require an indexer sidecar (deferred to v1.5); use transcript_snippet or clip_uuid instead".into(),
            });
        }
    };

    // Tier 1 → 4 against seeded metadata.
    for tier in [
        Tier::Exact,
        Tier::TrimEnd,
        Tier::TrimBoth,
        Tier::UnicodeNorm,
    ] {
        let normalized_needle = normalize(needle, tier);
        for clip in &clips {
            if let Some(haystack) = clip_text(clip)
                && normalize(&haystack, tier).contains(&normalized_needle)
            {
                return Ok(clip.locator);
            }
        }
    }

    // Tier 5: search whisper sidecars when we have a project_root.
    if let Some(loc) = resolve_via_whisper(&clips, needle, ctx) {
        return Ok(loc);
    }

    // Miss. Build candidates.
    let candidates = nearest_candidates_from_clips_and_sidecars(needle, &clips, ctx, 3);
    Err(AnchorMiss {
        anchor: anchor.clone(),
        candidates,
        reason: format!(
            "no clip found whose transcript snippet contains {needle:?} \
             (tried exact, whitespace-tolerant, unicode-normalized, \
             and whisper-sidecar matches)"
        ),
    })
}

/// Walk every clip's whisper sidecar looking for a segment whose text
/// contains the needle (4-tier cascade). On a hit, return the
/// locator of the clip that contains the matching segment's
/// timestamp inside its source_range. We pick the *first* clip whose
/// asset_id matches the segment's owning sidecar; if multiple clips
/// reference the same asset, we further filter by source_range
/// containment (start_s..start_s+duration covers the segment start).
fn resolve_via_whisper(
    clips: &[ClipEntry<'_>],
    needle: &str,
    ctx: &AnchorContext,
) -> Option<ClipLocator> {
    if ctx.project_root.is_none() {
        return None;
    }
    for tier in [
        Tier::Exact,
        Tier::TrimEnd,
        Tier::TrimBoth,
        Tier::UnicodeNorm,
    ] {
        let normalized_needle = normalize(needle, tier);
        for clip in clips {
            let asset_id = match &clip.asset_id {
                Some(id) => id,
                None => continue,
            };
            let segments = ctx.segments_for(asset_id);
            if segments.is_empty() {
                continue;
            }
            for seg in &segments {
                if normalize(&seg.text, tier).contains(&normalized_needle)
                    && clip_covers_segment(clip, seg)
                {
                    return Some(clip.locator);
                }
            }
        }
    }
    None
}

/// True iff the clip's source_range OVERLAPS the segment's time range
/// at all. Conservative-on-overlap, generous-on-boundary: the previous
/// strict-containment check (segment start must fall inside the clip
/// range) failed every Insert→Trim flow on real video because the
/// agent picks round-ish editorial boundaries (19.4s) while whisper
/// segments start mid-word (19.375s) — a 25ms mismatch was enough to
/// kill the anchor, surfacing as "anchor not found" on every freshly-
/// inserted clip in the 44-min real-video session.
///
/// Real-video evidence: whisper segments crossing the clip boundary
/// by tens of milliseconds is the norm, not the exception. Overlap is
/// the right semantic — if any of the segment's text appears anywhere
/// inside the clip's window, that segment IS what the clip is about.
///
/// Multi-clip-per-asset (e.g. previously-split clips) still works
/// because each candidate is evaluated independently; if a segment
/// overlaps both clips A and B, the resolver picks the first match in
/// the iteration order, which the existing 4-tier cascade already
/// disambiguates by score.
fn clip_covers_segment(clip: &ClipEntry<'_>, seg: &TranscriptSegment) -> bool {
    let Some(range) = clip.source_range else {
        return true; // no range info; be permissive
    };
    let clip_start = range.0;
    let clip_end = range.0 + range.1;
    // Overlap test: NOT (seg ends before clip starts OR seg starts
    // after clip ends).
    !(seg.end_s <= clip_start || seg.start_s >= clip_end)
}

fn nearest_candidates_from_clips_and_sidecars(
    needle: &str,
    clips: &[ClipEntry<'_>],
    ctx: &AnchorContext,
    k: usize,
) -> Vec<String> {
    let needle_l = needle.to_lowercase();
    let mut scored: Vec<(usize, String)> = clips
        .iter()
        .filter_map(|c| {
            c.transcript_snippet
                .map(|t| (overlap_score(&needle_l, &t.to_lowercase()), t.to_string()))
        })
        .collect();
    // If we have a project root, pull candidate phrases from whisper too.
    if ctx.project_root.is_some() {
        let mut seen_assets = std::collections::HashSet::new();
        for clip in clips {
            let Some(asset_id) = &clip.asset_id else {
                continue;
            };
            if !seen_assets.insert(asset_id.clone()) {
                continue;
            }
            for seg in ctx.segments_for(asset_id) {
                let s = (
                    overlap_score(&needle_l, &seg.text.to_lowercase()),
                    seg.text.clone(),
                );
                scored.push(s);
            }
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().take(k).map(|(_, s)| s).collect()
}

/// What we couldn't find. Surfaced to the agent as a `RespondToModel`
/// string with the candidates inlined.
#[derive(Debug, Clone)]
pub struct AnchorMiss {
    /// The anchor we tried to resolve.
    pub anchor: Anchor,
    /// Closest candidates (≤3) the model can pick from in a follow-up.
    pub candidates: Vec<String>,
    /// Why we missed.
    pub reason: String,
}

impl std::fmt::Display for AnchorMiss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "anchor not found: {} ({}).", self.anchor, self.reason)?;
        if !self.candidates.is_empty() {
            write!(f, " Did you mean one of:")?;
            for c in &self.candidates {
                write!(f, "\n  - {c:?}")?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum Tier {
    /// Match raw `text.contains(needle)`.
    Exact,
    /// Strip trailing whitespace on both sides.
    TrimEnd,
    /// Strip leading + trailing whitespace.
    TrimBoth,
    /// Replace smart quotes / em-dash / NBSP / ellipsis with ASCII
    /// equivalents.
    UnicodeNorm,
}

fn normalize(s: &str, tier: Tier) -> String {
    let s = match tier {
        Tier::Exact => s.to_string(),
        Tier::TrimEnd => s.trim_end().to_string(),
        Tier::TrimBoth => s.trim().to_string(),
        Tier::UnicodeNorm => unicode_fold(s),
    };
    s
}

fn unicode_fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\u{2018}' | '\u{2019}' => out.push('\''), // smart single quotes
            '\u{201c}' | '\u{201d}' => out.push('"'),  // smart double quotes
            '\u{2013}' | '\u{2014}' => out.push('-'),  // en-dash / em-dash
            '\u{00a0}' => out.push(' '),               // non-breaking space
            '\u{2026}' => out.push_str("..."),         // ellipsis
            other => out.push(other.to_ascii_lowercase()),
        }
    }
    out
}

/// One clip with the text we'll match against.
struct ClipEntry<'a> {
    locator: ClipLocator,
    name: &'a str,
    transcript_snippet: Option<&'a str>,
    awidat_uuid: Option<String>,
    marker_notes: Vec<&'a str>,
    /// External asset id (e.g. `raw/clip-1.MOV`). Used by the whisper
    /// sidecar lookup to find the matching transcript file.
    asset_id: Option<String>,
    /// Clip source_range as `(start_s, duration_s)` in seconds. Used
    /// to filter sidecar segments to those that fall inside the
    /// clip's media slice.
    source_range: Option<(f64, f64)>,
}

fn collect_clips(timeline: &Timeline) -> Vec<ClipEntry<'_>> {
    use awidat_proto::otio::{ExternalReference, MediaReference};
    let mut out = Vec::new();
    for (ti, sc) in timeline.tracks.children.iter().enumerate() {
        let StackChild::Track(track) = sc else {
            continue;
        };
        for (ci, tc) in track.children.iter().enumerate() {
            if let TrackChild::Clip(c) = tc {
                let snippet = c
                    .metadata
                    .awidat
                    .as_ref()
                    .and_then(|m| m.anchor.as_ref())
                    .and_then(|a| a.transcript_snippet.as_deref());
                let uuid = c
                    .metadata
                    .awidat
                    .as_ref()
                    .and_then(|m| m.extra.get("clip_uuid"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let marker_notes: Vec<&str> = c
                    .markers
                    .iter()
                    .filter_map(|m| m.metadata.awidat.as_ref().and_then(|am| am.note.as_deref()))
                    .collect();
                // Asset id from the external media reference's
                // target_url. The awidat anchor doesn't carry this
                // separately — clips on a track always resolve their
                // media via `media_reference`.
                let asset_id = match &c.media_reference {
                    MediaReference::External(ExternalReference { target_url, .. }) => {
                        Some(target_url.clone())
                    }
                    _ => None,
                };
                let source_range = c
                    .source_range
                    .as_ref()
                    .map(|r| (r.start_time.to_seconds(), r.duration.to_seconds()));
                out.push(ClipEntry {
                    locator: ClipLocator {
                        track_index: ti,
                        child_index: ci,
                    },
                    name: c.name.as_str(),
                    transcript_snippet: snippet,
                    awidat_uuid: uuid,
                    marker_notes,
                    asset_id,
                    source_range,
                });
            }
        }
    }
    out
}

fn resolve_by_uuid(clips: &[ClipEntry<'_>], uuid: &str) -> Result<ClipLocator, AnchorMiss> {
    for clip in clips {
        if clip.awidat_uuid.as_deref() == Some(uuid) || clip.name == uuid {
            return Ok(clip.locator);
        }
    }

    let asset_matches = clips
        .iter()
        .filter(|clip| {
            clip.asset_id
                .as_deref()
                .is_some_and(|asset_id| asset_reference_matches(asset_id, uuid))
        })
        .collect::<Vec<_>>();
    if asset_matches.len() == 1 {
        return Ok(asset_matches[0].locator);
    }

    let candidates = clips
        .iter()
        .flat_map(|c| {
            let mut values = Vec::new();
            if let Some(uuid) = &c.awidat_uuid {
                values.push(uuid.clone());
            }
            if !c.name.is_empty() {
                values.push(c.name.to_string());
            }
            if let Some(asset_id) = &c.asset_id {
                values.push(asset_id.clone());
            }
            values
        })
        .take(3)
        .collect::<Vec<_>>();
    let reason = if asset_matches.len() > 1 {
        format!(
            "uuid '{uuid}' matched {} clips by asset filename/stem; use the clip name from view_timeline instead",
            asset_matches.len()
        )
    } else {
        format!(
            "no clip with uuid '{uuid}' (matched against awidat.clip_uuid, clip name, and unique media filename/stem)"
        )
    };
    Err(AnchorMiss {
        anchor: Anchor::ClipUuid {
            uuid: uuid.to_string(),
        },
        candidates,
        reason,
    })
}

fn asset_reference_matches(asset_id: &str, needle: &str) -> bool {
    if asset_id == needle {
        return true;
    }
    let path = Path::new(asset_id);
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|name| name == needle)
        || path
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|stem| stem == needle)
}

fn clip_text(clip: &ClipEntry<'_>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(s) = clip.transcript_snippet {
        parts.push(s.to_string());
    }
    parts.extend(clip.marker_notes.iter().map(|s| s.to_string()));
    if !clip.name.is_empty() {
        parts.push(clip.name.to_string());
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join(" \n "))
}

/// Crude overlap score: count of needle words that appear in the
/// haystack. Cheap and roughly right for "did you mean?" suggestions.
fn overlap_score(needle: &str, haystack: &str) -> usize {
    let h_words: std::collections::HashSet<&str> = haystack.split_whitespace().collect();
    needle
        .split_whitespace()
        .filter(|w| h_words.contains(w))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::awidat_meta::{Anchor as AwAnchor, AwidatClipMetadata};
    use awidat_proto::otio::{
        Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
        Timeline, Track, TrackChild, TrackKind,
    };

    fn timeline_with(snippets: &[&str]) -> Timeline {
        let mut tl = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        for (i, snippet) in snippets.iter().enumerate() {
            let mut clip = Clip::empty(format!("clip-{i}"));
            clip.media_reference =
                MediaReference::External(ExternalReference::new(format!("raw/{i}.mp4")));
            clip.source_range = Some(TimeRange::new(
                RationalTime::zero(24.0),
                RationalTime::new(24.0, 24.0),
            ));
            clip.metadata = ClipMetadata {
                awidat: Some(AwidatClipMetadata {
                    anchor: Some(AwAnchor {
                        transcript_snippet: Some((*snippet).to_string()),
                        ..AwAnchor::default()
                    }),
                    ..AwidatClipMetadata::default()
                }),
                ..ClipMetadata::default()
            };
            track.children.push(TrackChild::Clip(clip));
        }
        tl.tracks.children.push(StackChild::Track(track));
        tl
    }

    #[test]
    fn exact_substring_match_resolves() {
        let tl = timeline_with(&[
            "first clip text",
            "and that's when she said the thing about Stripe",
            "third clip",
        ]);
        let loc = resolve(
            &tl,
            &Anchor::TranscriptSnippet {
                text: "thing about Stripe".into(),
            },
            &AnchorContext::empty(),
        )
        .unwrap();
        assert_eq!(loc.child_index, 1);
    }

    #[test]
    fn whitespace_tolerant_match() {
        let tl = timeline_with(&["  hello  world  "]);
        // Needle has different surrounding whitespace.
        let loc = resolve(
            &tl,
            &Anchor::TranscriptSnippet {
                text: "hello  world".into(),
            },
            &AnchorContext::empty(),
        )
        .unwrap();
        assert_eq!(loc.child_index, 0);
    }

    #[test]
    fn smart_quotes_match_via_unicode_fold() {
        // Haystack uses smart quotes; needle uses ASCII.
        let tl = timeline_with(&["she said \u{201c}stripe is great\u{201d}"]);
        let loc = resolve(
            &tl,
            &Anchor::TranscriptSnippet {
                text: "\"stripe is great\"".into(),
            },
            &AnchorContext::empty(),
        )
        .unwrap();
        assert_eq!(loc.child_index, 0);
    }

    #[test]
    fn miss_returns_top_3_candidates() {
        let tl = timeline_with(&[
            "the rain in spain falls mainly on the plain",
            "spain is sunny most days",
            "rain forecast for tomorrow",
            "completely unrelated",
        ]);
        let err = resolve(
            &tl,
            &Anchor::TranscriptSnippet {
                text: "spain weather is variable".into(),
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        // top candidates should include the spain-bearing clips
        assert_eq!(err.candidates.len().min(3), 3);
        assert!(err.candidates.iter().any(|c| c.contains("spain")));
    }

    #[test]
    fn miss_message_is_actionable() {
        let tl = timeline_with(&["hello"]);
        let err = resolve(
            &tl,
            &Anchor::TranscriptSnippet {
                text: "goodbye".into(),
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("anchor not found"));
        assert!(msg.contains("\"goodbye\""));
        assert!(msg.contains("Did you mean"));
    }

    #[test]
    fn clip_uuid_anchor_matches_extra_field() {
        let mut tl = timeline_with(&["foo"]);
        // Inject a uuid via the awidat extra map.
        let StackChild::Track(t) = &mut tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &mut t.children[0] else {
            panic!()
        };
        c.metadata.awidat.as_mut().unwrap().extra.insert(
            "clip_uuid".into(),
            serde_json::Value::String("c-9f2".into()),
        );

        let loc = resolve(
            &tl,
            &Anchor::ClipUuid {
                uuid: "c-9f2".into(),
            },
            &AnchorContext::empty(),
        )
        .unwrap();
        assert_eq!(loc.child_index, 0);
    }

    #[test]
    fn clip_uuid_falls_back_to_clip_name() {
        let tl = timeline_with(&["foo"]);
        let loc = resolve(
            &tl,
            &Anchor::ClipUuid {
                uuid: "clip-0".into(),
            },
            &AnchorContext::empty(),
        )
        .unwrap();
        assert_eq!(loc.child_index, 0);
    }

    #[test]
    fn clip_uuid_falls_back_to_unique_asset_filename_stem() {
        let mut tl = timeline_with(&["foo"]);
        let StackChild::Track(t) = &mut tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &mut t.children[0] else {
            panic!()
        };
        c.media_reference = MediaReference::External(ExternalReference::new(
            "raw/copy_F65206FA-9AEC-4CA8-BD2F-ACD63775F7FA.MOV",
        ));

        let loc = resolve(
            &tl,
            &Anchor::ClipUuid {
                uuid: "copy_F65206FA-9AEC-4CA8-BD2F-ACD63775F7FA".into(),
            },
            &AnchorContext::empty(),
        )
        .unwrap();
        assert_eq!(loc.child_index, 0);
    }

    #[test]
    fn clip_uuid_asset_fallback_rejects_ambiguous_asset_matches() {
        let mut tl = timeline_with(&["foo", "bar"]);
        let StackChild::Track(t) = &mut tl.tracks.children[0] else {
            panic!()
        };
        for child in &mut t.children {
            let TrackChild::Clip(c) = child else { panic!() };
            c.media_reference = MediaReference::External(ExternalReference::new("raw/shared.MOV"));
        }

        let err = resolve(
            &tl,
            &Anchor::ClipUuid {
                uuid: "shared".into(),
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        assert!(err.reason.contains("matched 2 clips"));
    }

    #[test]
    fn scene_change_anchor_is_v1_5() {
        let tl = timeline_with(&["foo"]);
        let err = resolve(
            &tl,
            &Anchor::SceneChangeIndex {
                asset_id: "raw/x.mp4".into(),
                index: 0,
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        assert!(err.reason.contains("v1.5"));
    }

    #[test]
    fn empty_timeline_produces_no_candidates() {
        let tl = Timeline::empty("empty");
        let err = resolve(
            &tl,
            &Anchor::TranscriptSnippet {
                text: "anything".into(),
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        assert!(err.candidates.is_empty());
    }

    #[test]
    fn first_match_wins_when_multiple_overlap() {
        let tl = timeline_with(&["shared word here", "shared word also here"]);
        let loc = resolve(
            &tl,
            &Anchor::TranscriptSnippet {
                text: "shared word".into(),
            },
            &AnchorContext::empty(),
        )
        .unwrap();
        // Tier 1 (Exact) returns the first hit in iteration order.
        assert_eq!(loc.child_index, 0);
    }

    /// The headline test for #64: the agent passes a phrase that's
    /// nowhere in the seeded clip metadata, but lives inside the
    /// whisper sidecar's transcript. With an `AnchorContext` rooted
    /// at the project, the resolver should find the right clip.
    #[test]
    fn whisper_sidecar_search_resolves_phrase_outside_clip_metadata() {
        use awidat_proto::otio::{Clip, ExternalReference, MediaReference};
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();

        // Build a one-clip timeline whose clip-level anchor is just
        // an asset id (the realistic shape after `awidat init` and a
        // manual seed). No transcript_snippet seeded.
        let mut tl = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("clip-0".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/clip-1.MOV"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(56.0 * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(clip));
        tl.tracks.children.push(StackChild::Track(track));

        // Drop a synthetic whisper sidecar at the canonical path.
        let whisper_dir = project_root.join("index/whisper/raw");
        std::fs::create_dir_all(&whisper_dir).unwrap();
        let body = serde_json::json!({
            "indexer": "whisper",
            "asset_id": "raw/clip-1.MOV",
            "data": {
                "language": "en",
                "segments": [
                    {"text": "Hello and welcome to Builders Network",
                     "start_s": 0.0, "end_s": 3.0},
                    {"text": "we connect founders with talent",
                     "start_s": 3.0, "end_s": 6.0},
                ]
            }
        });
        std::fs::write(
            whisper_dir.join("clip-1.MOV.json"),
            serde_json::to_vec_pretty(&body).unwrap(),
        )
        .unwrap();

        // Empty context: no project root → the resolver can't see
        // the sidecar → miss.
        let err = resolve(
            &tl,
            &Anchor::TranscriptSnippet {
                text: "Builders Network".into(),
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        assert!(err.candidates.is_empty(), "no metadata, no candidates");

        // Project-rooted context → resolver loads the sidecar and
        // finds the segment, mapping it back to clip-0.
        let ctx = AnchorContext::with_project_root(project_root);
        let loc = resolve(
            &tl,
            &Anchor::TranscriptSnippet {
                text: "Builders Network".into(),
            },
            &ctx,
        )
        .expect("sidecar search should resolve");
        assert_eq!(loc.child_index, 0);
    }

    /// Regression test for the bug found in the 44-min real-video
    /// session: an `Insert Clip` op produces a clip whose source_range
    /// is at editorially-rounded boundaries (e.g. 19.4s, 146s,
    /// 742.3s), but whisper segments start mid-word at unrelated
    /// timestamps (19.375s, 145.95s, 744.758s). The previous
    /// `clip_covers_segment` used strict containment
    /// (`seg.start_s >= clip_start && seg.start_s < clip_end`) — a
    /// 25ms boundary mismatch was enough to fail the anchor lookup,
    /// breaking every Insert→Trim flow on real footage.
    ///
    /// The fix replaces strict containment with overlap: if the
    /// segment's text-time-window overlaps the clip's source_range
    /// at all, that segment IS what the clip is about. Editorial
    /// rounding shouldn't kill anchoring.
    #[test]
    fn whisper_anchor_resolves_when_segment_starts_just_before_clip() {
        use awidat_proto::otio::{Clip, ExternalReference, MediaReference};
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();

        // Inserted clip at [19.4, 34.6] — round-ish editorial bounds.
        let mut tl = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("clip-0".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/ep.mp4"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(19.4 * 24.0, 24.0),
            RationalTime::new((34.6 - 19.4) * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(clip));
        tl.tracks.children.push(StackChild::Track(track));

        // Whisper segment starts at 19.375s — 25ms BEFORE clip_start.
        // Pre-fix `clip_covers_segment` would reject this; post-fix
        // the overlap test accepts it because the segment ends at
        // 26.5s, well inside the clip range.
        let whisper_dir = project_root.join("index/whisper/raw");
        std::fs::create_dir_all(&whisper_dir).unwrap();
        let body = serde_json::json!({
            "indexer": "whisper",
            "asset_id": "raw/ep.mp4",
            "data": {
                "language": "en",
                "segments": [
                    {"text": "Samsung Galaxy S. One of the most popular names in the world of smartphones.",
                     "start_s": 19.375, "end_s": 26.5},
                    {"text": "Interestingly, it's kind of a long name.",
                     "start_s": 26.579, "end_s": 28.0},
                ]
            }
        });
        std::fs::write(
            whisper_dir.join("ep.mp4.json"),
            serde_json::to_vec_pretty(&body).unwrap(),
        )
        .unwrap();

        let ctx = AnchorContext::with_project_root(project_root);
        let loc = resolve(
            &tl,
            &Anchor::TranscriptSnippet {
                text: "One of the most popular names".into(),
            },
            &ctx,
        )
        .expect(
            "overlap-tolerant resolution should match a segment that \
             starts just before the clip's editorially-rounded start",
        );
        assert_eq!(loc.child_index, 0);
    }

    /// Companion regression: a segment that ends just AFTER the
    /// clip ends should still match. Same overlap semantics, other
    /// boundary.
    #[test]
    fn whisper_anchor_resolves_when_segment_ends_just_after_clip() {
        use awidat_proto::otio::{Clip, ExternalReference, MediaReference};
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();

        let mut tl = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("clip-0".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/ep.mp4"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(742.0 * 24.0, 24.0),
            RationalTime::new(9.0 * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(clip));
        tl.tracks.children.push(StackChild::Track(track));

        let whisper_dir = project_root.join("index/whisper/raw");
        std::fs::create_dir_all(&whisper_dir).unwrap();
        let body = serde_json::json!({
            "indexer": "whisper",
            "asset_id": "raw/ep.mp4",
            "data": {
                "language": "en",
                "segments": [
                    // Segment starts inside the clip but ends 0.4s
                    // after the clip's end. Overlaps; should match.
                    {"text": "phone battery swelling, catching fire and burning up",
                     "start_s": 750.0, "end_s": 751.4},
                ]
            }
        });
        std::fs::write(
            whisper_dir.join("ep.mp4.json"),
            serde_json::to_vec_pretty(&body).unwrap(),
        )
        .unwrap();

        let ctx = AnchorContext::with_project_root(project_root);
        let loc = resolve(
            &tl,
            &Anchor::TranscriptSnippet {
                text: "battery swelling".into(),
            },
            &ctx,
        )
        .expect(
            "overlap-tolerant resolution should match a segment that crosses the trailing boundary",
        );
        assert_eq!(loc.child_index, 0);
    }

    /// And the negation: a segment that ends BEFORE the clip starts
    /// (genuinely belongs to a different clip / the previous source
    /// region) must NOT match. This guards multi-clip-per-asset
    /// disambiguation.
    #[test]
    fn whisper_anchor_rejects_segment_outside_clip_range() {
        use awidat_proto::otio::{Clip, ExternalReference, MediaReference};
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();

        let mut tl = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("clip-0".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/ep.mp4"));
        // Clip is at [100, 110] — the segment below ends at 50.
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(100.0 * 24.0, 24.0),
            RationalTime::new(10.0 * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(clip));
        tl.tracks.children.push(StackChild::Track(track));

        let whisper_dir = project_root.join("index/whisper/raw");
        std::fs::create_dir_all(&whisper_dir).unwrap();
        let body = serde_json::json!({
            "indexer": "whisper",
            "asset_id": "raw/ep.mp4",
            "data": {
                "language": "en",
                "segments": [
                    {"text": "this happened way earlier in the episode",
                     "start_s": 45.0, "end_s": 50.0},
                ]
            }
        });
        std::fs::write(
            whisper_dir.join("ep.mp4.json"),
            serde_json::to_vec_pretty(&body).unwrap(),
        )
        .unwrap();

        let ctx = AnchorContext::with_project_root(project_root);
        let err = resolve(
            &tl,
            &Anchor::TranscriptSnippet {
                text: "way earlier".into(),
            },
            &ctx,
        )
        .unwrap_err();
        // Should NOT match — the segment ends 50s before the clip
        // even starts. Overlap test correctly rejects this.
        assert!(
            err.reason.contains("no clip found"),
            "expected miss; got {:?}",
            err.reason
        );
    }
}
