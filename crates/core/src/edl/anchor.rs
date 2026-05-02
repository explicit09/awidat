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
//!    `awidat.clip_uuid` extra.
//!
//! On miss, return up to 3 closest candidates by simple character-overlap
//! score (Aider's "did you mean?" pattern at
//! `aider/aider/coders/editblock_coder.py:91-106`). The agent gets these
//! in the `RespondToModel` error so it can self-correct in the same turn.

use awidat_proto::otio::{StackChild, Timeline, TrackChild};

use super::op::Anchor;

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
pub fn resolve(timeline: &Timeline, anchor: &Anchor) -> Result<ClipLocator, AnchorMiss> {
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

    // Tier 1 → 4: progressively-relaxed substring matches against any
    // available text on each clip.
    for tier in [Tier::Exact, Tier::TrimEnd, Tier::TrimBoth, Tier::UnicodeNorm] {
        let normalized_needle = normalize(needle, tier);
        for clip in &clips {
            if let Some(haystack) = clip_text(clip)
                && normalize(&haystack, tier).contains(&normalized_needle)
            {
                return Ok(clip.locator);
            }
        }
    }

    // Miss. Build candidates.
    let candidates = nearest_candidates(needle, &clips, 3);
    Err(AnchorMiss {
        anchor: anchor.clone(),
        candidates,
        reason: format!(
            "no clip found whose transcript snippet contains {needle:?} \
             (tried exact, whitespace-tolerant, and unicode-normalized matches)"
        ),
    })
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
            '\u{2018}' | '\u{2019}' => out.push('\''),    // smart single quotes
            '\u{201c}' | '\u{201d}' => out.push('"'),     // smart double quotes
            '\u{2013}' | '\u{2014}' => out.push('-'),     // en-dash / em-dash
            '\u{00a0}' => out.push(' '),                  // non-breaking space
            '\u{2026}' => out.push_str("..."),            // ellipsis
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
}

fn collect_clips(timeline: &Timeline) -> Vec<ClipEntry<'_>> {
    let mut out = Vec::new();
    for (ti, sc) in timeline.tracks.children.iter().enumerate() {
        let StackChild::Track(track) = sc else { continue };
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
                    .filter_map(|m| {
                        m.metadata
                            .awidat
                            .as_ref()
                            .and_then(|am| am.note.as_deref())
                    })
                    .collect();
                out.push(ClipEntry {
                    locator: ClipLocator {
                        track_index: ti,
                        child_index: ci,
                    },
                    name: c.name.as_str(),
                    transcript_snippet: snippet,
                    awidat_uuid: uuid,
                    marker_notes,
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
    let candidates = clips
        .iter()
        .filter_map(|c| c.awidat_uuid.clone())
        .take(3)
        .collect::<Vec<_>>();
    Err(AnchorMiss {
        anchor: Anchor::ClipUuid {
            uuid: uuid.to_string(),
        },
        candidates,
        reason: format!("no clip with uuid '{uuid}' (matched against awidat.clip_uuid and clip name)"),
    })
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

fn nearest_candidates(needle: &str, clips: &[ClipEntry<'_>], k: usize) -> Vec<String> {
    let needle_l = needle.to_lowercase();
    let mut scored: Vec<(usize, String)> = clips
        .iter()
        .filter_map(|c| c.transcript_snippet.map(|t| (overlap_score(&needle_l, &t.to_lowercase()), t.to_string())))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().take(k).map(|(_, s)| s).collect()
}

/// Crude overlap score: count of needle words that appear in the
/// haystack. Cheap and roughly right for "did you mean?" suggestions.
fn overlap_score(needle: &str, haystack: &str) -> usize {
    let h_words: std::collections::HashSet<&str> = haystack.split_whitespace().collect();
    needle.split_whitespace().filter(|w| h_words.contains(w)).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::awidat_meta::{Anchor as AwAnchor, AwidatClipMetadata};
    use awidat_proto::otio::{
        Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, Stack, StackChild,
        TimeRange, Timeline, Track, TrackChild, TrackKind,
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
        let StackChild::Track(t) = &mut tl.tracks.children[0] else { panic!() };
        let TrackChild::Clip(c) = &mut t.children[0] else { panic!() };
        c.metadata.awidat.as_mut().unwrap().extra.insert(
            "clip_uuid".into(),
            serde_json::Value::String("c-9f2".into()),
        );

        let loc = resolve(
            &tl,
            &Anchor::ClipUuid {
                uuid: "c-9f2".into(),
            },
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
        )
        .unwrap();
        assert_eq!(loc.child_index, 0);
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
        )
        .unwrap_err();
        assert!(err.candidates.is_empty());
    }

    #[test]
    fn first_match_wins_when_multiple_overlap() {
        let tl = timeline_with(&[
            "shared word here",
            "shared word also here",
        ]);
        let loc = resolve(
            &tl,
            &Anchor::TranscriptSnippet {
                text: "shared word".into(),
            },
        )
        .unwrap();
        // Tier 1 (Exact) returns the first hit in iteration order.
        assert_eq!(loc.child_index, 0);
    }
}
