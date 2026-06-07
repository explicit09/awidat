//! Shared clip-anchor helpers for plan-* tools that emit a
//! `clip_uuid=<anchor>` EDL anchor against the project timeline.
//!
//! Two pieces, both reused by `plan_color_grade` (and, by design, the
//! sibling `plan_speed_ramp`):
//!
//! - [`normalize_project_rel`] — canonicalize a project-relative path
//!   to the SAME shape `apply_edl`'s `normalize_lut_path` requires:
//!   strip a leading `./`, reject `..`/absolute/backslashes and any
//!   non-`Component::Normal` part, re-join with `/`. A clip sampled as
//!   `./raw/foo.mp4` must be emitted as `raw/foo.mp4` so it matches the
//!   timeline's `target_url`.
//!
//! - [`resolve_clip_anchor`] — pick the EDL anchor for an asset and the
//!   matched clip's source range. An explicit clip name/uuid (from
//!   `view_timeline`) is used verbatim, but VALIDATED against the
//!   timeline: it must resolve (same first-match rule as `apply_edl`) to a
//!   VIDEO clip whose media target is the asset. The project OTIO must be
//!   readable for BOTH explicit and implicit anchors (`apply_edl` needs it
//!   too). Otherwise we load the project OTIO and COUNT the VIDEO clips whose
//!   resolved media target equals the normalized asset path: exactly one
//!   → emit THAT clip's own id (its `awidat.clip_uuid`, unique per clip;
//!   the name only as a fallback) as the anchor, so `apply_edl` resolves it
//!   on its first pass (uuid/name match) — NOT the asset-match fallback,
//!   which counts the linked audio clip too and rejects as ambiguous, nor a
//!   colliding clip name. Zero or many video matches → fail loud at PLAN
//!   time rather than letting `apply_edl` reject (or mis-resolve) the anchor.
//!
//!   Only VIDEO-track clips are counted/matched: in a normal linked A/V
//!   timeline the picture clip and its audio clip both reference the same
//!   media file, so counting both would wrongly report two matches. Color
//!   grading anchors to the picture, so the audio half is irrelevant.
//!
//!   The matched clip's `source_range` (start + duration, in source/media
//!   seconds) is returned so callers sample WITHIN the trim rather than
//!   over the whole asset. It is `None` when the matched clip has no
//!   `source_range` — callers then fall back to the whole asset.

use std::path::{Component, Path};

use awidat_proto::otio::{Clip, MediaReference, StackChild, Timeline, TrackChild, TrackKind};

/// Canonicalize a project-relative path to the shape `apply_edl`'s
/// `normalize_lut_path` accepts: no leading `./`, no `..`, no absolute
/// prefix, no backslashes, every component a plain name. Returns the
/// `/`-joined normalized form, or `Err` for any un-normalizable
/// spelling.
pub fn normalize_project_rel(path: &str) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path must be a non-empty project-relative path".into());
    }
    if trimmed.contains('\\') {
        return Err(format!(
            "path {path:?} must not contain backslashes (use '/' separators)"
        ));
    }
    let p = Path::new(trimmed);
    if p.is_absolute() {
        return Err(format!(
            "path {path:?} must be a project-relative path (not absolute)"
        ));
    }
    let mut parts: Vec<&str> = Vec::new();
    for (i, component) in p.components().enumerate() {
        match component {
            // Only a LEADING `./` is dropped (the common spelling for a
            // project-relative path). A `.` anywhere else is rejected to
            // mirror apply_edl, which forbids every non-Normal component.
            Component::CurDir if i == 0 => continue,
            Component::Normal(os) => {
                let Some(s) = os.to_str() else {
                    return Err(format!("path {path:?} contains a non-UTF-8 component"));
                };
                parts.push(s);
            }
            // Interior CurDir, ParentDir, RootDir, Prefix → all rejected.
            _ => {
                return Err(format!(
                    "path {path:?} must not contain '.', '..', or absolute/root prefixes"
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(format!(
            "path {path:?} normalizes to an empty project-relative path"
        ));
    }
    Ok(parts.join("/"))
}

/// A resolved EDL anchor plus the matched clip's source range.
///
/// `source_range` is the matched VIDEO clip's `source_range` expressed as
/// `(start_s, duration_s)` in source/media seconds, or `None` when the
/// matched clip has no `source_range`. Callers that sample frames should
/// restrict sampling to this window and fall back to the whole asset when
/// it is `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAnchor {
    /// The EDL anchor (`clip_uuid=<anchor>`): the matched clip's
    /// `awidat.clip_uuid` (or its name) — never the asset path, so
    /// `apply_edl` resolves it on its first pass.
    pub anchor: String,
    /// The matched clip's source range, `(start_s, duration_s)`, in
    /// source/media seconds, when determinable.
    pub source_range: Option<(f64, f64)>,
}

/// Resolve the EDL anchor for `asset_rel` (already normalized via
/// [`normalize_project_rel`]) and the matched clip's source range.
///
/// Only VIDEO-track clips are counted/matched — a linked A/V pair both
/// reference the same media, and the grade anchors to the picture.
///
/// - `explicit` (the clip name/uuid from `view_timeline`): used as the
///   anchor verbatim (it is a NAME, not a path — NOT normalized), but
///   VALIDATED against the timeline (which must be readable — `apply_edl`
///   needs it). Resolved to the FIRST video clip (timeline order) matching
///   the name/uuid, exactly like `apply_edl`'s `clip_id`; THAT clip's
///   media target must be `asset_rel`, else `Err` (an earlier same-named
///   clip on a different asset is what `apply_edl` would stamp).
/// - `None`: load `<project_root>/project.otio.json` and count video
///   clips whose media target equals `asset_rel`. Exactly one → emit THAT
///   clip's id (`awidat.clip_uuid`, or its name) as the anchor — apply_edl
///   resolves it on the first pass, never the audio-counting asset fallback.
///   Zero → `Err`. More than one → `Err` naming the count.
pub fn resolve_clip_anchor(
    project_root: &Path,
    asset_rel: &str,
    explicit: Option<&str>,
) -> Result<ResolvedAnchor, String> {
    // The project timeline is required for BOTH paths: the advertised next
    // step `apply_edl` calls `Project::read` and fails without it, so fail
    // loud here rather than handing back an anchor apply_edl can't honor
    // (finding 3364995218).
    let timeline = load_project_timeline(project_root)?;

    if let Some(name) = explicit {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("clip_anchor must not be empty".into());
        }
        reject_control_chars("clip_anchor", trimmed)?;
        // Mirror apply_edl's resolver exactly (anchor.rs `resolve_by_uuid`):
        // the FIRST clip across ALL tracks (track, then clip order) whose
        // uuid OR name equals the anchor wins — then require THAT clip to be
        // a video clip referencing the requested asset (findings 3364995223,
        // 3366562451). Scanning all tracks (not video-only) matters because
        // an earlier AUDIO clip sharing the name/uuid is what apply_edl would
        // stamp.
        let clip = resolve_anchor_to_video_asset(&timeline, trimmed, asset_rel)?;
        return Ok(ResolvedAnchor {
            anchor: trimmed.to_string(),
            source_range: clip_source_range(clip),
        });
    }

    // No explicit anchor: there must be exactly one VIDEO clip referencing
    // the asset (a linked A/V pair shares the media, so audio is skipped).
    let video_matches: Vec<&Clip> = video_clips_referencing(&timeline, asset_rel);
    let clip = match video_matches.as_slice() {
        [clip] => *clip,
        [] => {
            return Err(format!(
                "no timeline clip references {asset_rel:?}; pass clip_anchor with the clip name from view_timeline, or add the clip to the timeline first"
            ));
        }
        many => {
            return Err(format!(
                "{asset_rel:?} appears in {} clips; pass clip_anchor with the clip name from view_timeline to disambiguate",
                many.len()
            ));
        }
    };
    // Emit the matched clip's OWN id (its `awidat.clip_uuid`, unique per
    // clip; the name only as a last resort) rather than the asset path.
    // apply_edl then resolves it on the first pass (uuid/name match) instead
    // of the asset-match fallback, which counts the linked audio clip too
    // and rejects as "ambiguous" (finding 3366562448), and avoids colliding
    // with another clip's name (finding 3366562450).
    let anchor_id = clip_anchor_id(clip);
    reject_control_chars("clip name/uuid", anchor_id)?;
    // Guard that the emitted id is unambiguous under apply_edl's all-track
    // first-match (a name-only clip could be shadowed by an earlier clip).
    let resolved = resolve_anchor_to_video_asset(&timeline, anchor_id, asset_rel).map_err(|e| {
        format!(
            "{e}; the clip's identifier is ambiguous — re-stamp the clip's uuid or pass clip_anchor"
        )
    })?;
    Ok(ResolvedAnchor {
        anchor: anchor_id.to_string(),
        source_range: clip_source_range(resolved),
    })
}

/// Reject anchors carrying control characters (newlines, tabs, NUL…). The
/// EDL `@@ anchor:` line is line-oriented, so an embedded newline would make
/// `run()` return an EDL whose anchor no longer parses as validated, or that
/// apply_edl interprets as extra lines (finding 3366562452).
fn reject_control_chars(label: &str, value: &str) -> Result<(), String> {
    if value.chars().any(char::is_control) {
        return Err(format!(
            "{label} {value:?} must not contain control characters (newlines/tabs)"
        ));
    }
    Ok(())
}

/// The clip's emit-side anchor id: its `awidat.clip_uuid` when present
/// (unique per clip), else its name.
fn clip_anchor_id(clip: &Clip) -> &str {
    clip_uuid(clip).unwrap_or(clip.name.as_str())
}

/// Resolve `id` the way apply_edl does — the FIRST clip across ALL tracks
/// (track order, then clip order) whose uuid OR name equals `id` — and
/// require that clip to be a VIDEO clip referencing `asset_rel`. Errors
/// otherwise, mirroring what apply_edl would (mis)stamp.
fn resolve_anchor_to_video_asset<'a>(
    timeline: &'a Timeline,
    id: &str,
    asset_rel: &str,
) -> Result<&'a Clip, String> {
    match first_clip_matching(timeline, id) {
        None => Err(format!(
            "clip_anchor {id:?} does not match any video clip referencing {asset_rel:?}; pass the exact clip name/uuid from view_timeline"
        )),
        Some((clip, is_video)) if !is_video => {
            let _ = clip;
            Err(format!(
                "clip_anchor {id:?} resolves to a non-video clip; color grading anchors to the picture — pass the video clip's name/uuid from view_timeline"
            ))
        }
        Some((clip, _)) if clip_media_target(clip) != Some(asset_rel) => Err(format!(
            "clip_anchor {id:?} resolves to a clip referencing {:?}, not {asset_rel:?}; apply_edl would stamp that clip — pass the exact clip name/uuid for the requested asset",
            clip_media_target(clip).unwrap_or("<missing media>")
        )),
        Some((clip, _)) => Ok(clip),
    }
}

/// First clip across ALL tracks (track order, then clip order) matching
/// `id` by uuid-or-name, with whether it sits on a VIDEO track. Mirrors the
/// scan order apply_edl's `resolve_by_uuid` uses.
fn first_clip_matching<'a>(timeline: &'a Timeline, id: &str) -> Option<(&'a Clip, bool)> {
    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        let is_video = track.kind == TrackKind::Video;
        for track_child in &track.children {
            if let TrackChild::Clip(clip) = track_child
                && clip_matches_anchor(clip, id)
            {
                return Some((clip, is_video));
            }
        }
    }
    None
}

/// Load `<project_root>/project.otio.json` using the same loader
/// `apply_edl`/`verify` use, so the parsed model matches what the
/// resolver sees at apply time.
fn load_project_timeline(project_root: &Path) -> Result<Timeline, String> {
    let path = project_root.join("project.otio.json");
    let mut warnings = Vec::new();
    awidat_proto::project::read_otio_timeline(&path, &mut warnings).map_err(|e| {
        format!(
            "could not read timeline at {} (run from a project with a committed timeline): {e}",
            path.display()
        )
    })
}

/// All VIDEO-track clips whose resolved external media target equals
/// `asset_rel`. Audio tracks are skipped so a linked A/V pair counts once.
fn video_clips_referencing<'a>(timeline: &'a Timeline, asset_rel: &str) -> Vec<&'a Clip> {
    let mut matches = Vec::new();
    for_each_video_clip(timeline, |clip| {
        if clip_media_target(clip) == Some(asset_rel) {
            matches.push(clip);
        }
    });
    matches
}

/// Visit every clip on a VIDEO track.
fn for_each_video_clip<'a>(timeline: &'a Timeline, mut visit: impl FnMut(&'a Clip)) {
    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        if track.kind != TrackKind::Video {
            continue;
        }
        for track_child in &track.children {
            if let TrackChild::Clip(clip) = track_child {
                visit(clip);
            }
        }
    }
}

/// Whether `anchor` matches this clip's uuid or name — mirrors
/// `apply.rs`'s `clip_id` (uuid wins, falling back to the clip name).
fn clip_matches_anchor(clip: &Clip, anchor: &str) -> bool {
    clip_uuid(clip) == Some(anchor) || clip.name == anchor
}

/// The clip's `awidat.clip_uuid` metadata string, if present. Mirrors
/// `apply.rs`'s private `clip_uuid`.
fn clip_uuid(clip: &Clip) -> Option<&str> {
    clip.metadata
        .awidat
        .as_ref()
        .and_then(|m| m.extra.get("clip_uuid"))
        .and_then(|v| v.as_str())
}

/// The clip's source range as `(start_s, duration_s)` in source seconds,
/// or `None` when the clip has no `source_range`.
fn clip_source_range(clip: &Clip) -> Option<(f64, f64)> {
    clip.source_range
        .map(|range| (range.start_time.to_seconds(), range.duration.to_seconds()))
}

/// External media `target_url` for a clip, or `None` for a missing
/// reference. Mirrors `apply.rs`'s private `clip_media_target`.
fn clip_media_target(clip: &Clip) -> Option<&str> {
    match &clip.media_reference {
        MediaReference::External(reference) => Some(reference.target_url.as_str()),
        MediaReference::Missing(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, StackChild, Timeline, Track, TrackChild, TrackKind,
    };

    // --- normalize_project_rel ---

    #[test]
    fn normalize_strips_leading_dot_slash() {
        assert_eq!(normalize_project_rel("./raw/x.mp4").unwrap(), "raw/x.mp4");
    }

    #[test]
    fn normalize_passes_clean_relative_path() {
        assert_eq!(normalize_project_rel("raw/x.mp4").unwrap(), "raw/x.mp4");
    }

    #[test]
    fn normalize_rejects_backslashes() {
        assert!(normalize_project_rel("raw\\x.mp4").is_err());
    }

    #[test]
    fn normalize_rejects_parent_dir() {
        assert!(normalize_project_rel("../x.mp4").is_err());
        assert!(normalize_project_rel("raw/../x.mp4").is_err());
    }

    #[test]
    fn normalize_rejects_absolute() {
        assert!(normalize_project_rel("/tmp/x.mp4").is_err());
    }

    #[test]
    fn normalize_rejects_empty() {
        assert!(normalize_project_rel("").is_err());
        assert!(normalize_project_rel("   ").is_err());
        assert!(normalize_project_rel(".").is_err());
    }

    // --- resolve_clip_anchor ---

    use awidat_proto::otio::{RationalTime, TimeRange};

    /// Write a minimal `project.otio.json` whose single VIDEO track holds
    /// one clip per asset path in `assets`.
    fn write_timeline(root: &Path, assets: &[&str]) {
        let mut tl = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        for (i, asset) in assets.iter().enumerate() {
            let mut clip = Clip::empty(format!("clip-{i}"));
            clip.media_reference = MediaReference::External(ExternalReference::new(*asset));
            track.children.push(TrackChild::Clip(clip));
        }
        tl.tracks.children.push(StackChild::Track(track));
        write_otio(root, &tl);
    }

    fn write_otio(root: &Path, tl: &Timeline) {
        let json = serde_json::to_vec_pretty(tl).unwrap();
        std::fs::write(root.join("project.otio.json"), json).unwrap();
    }

    fn external_clip(name: &str, asset: &str) -> Clip {
        let mut clip = Clip::empty(name);
        clip.media_reference = MediaReference::External(ExternalReference::new(asset));
        clip
    }

    fn track_with(kind: TrackKind, name: &str, clips: Vec<Clip>) -> Track {
        let mut track = Track::empty(name, kind);
        for clip in clips {
            track.children.push(TrackChild::Clip(clip));
        }
        track
    }

    /// Finding 3364995218: an explicit anchor with NO `project.otio.json`
    /// on disk must FAIL LOUD — the advertised next step `apply_edl` calls
    /// `Project::read` and fails without that timeline, so the planner must
    /// not hand back a verbatim anchor that apply_edl can't honor.
    #[test]
    fn explicit_anchor_without_timeline_errors() {
        let tmp = tempfile::tempdir().unwrap();
        // No project.otio.json on disk — must fail, not pass through.
        let err =
            resolve_clip_anchor(tmp.path(), "raw/x.mp4", Some("Interview Clip 3")).unwrap_err();
        assert!(err.contains("could not read timeline"), "unexpected: {err}");
    }

    #[test]
    fn explicit_anchor_rejects_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(resolve_clip_anchor(tmp.path(), "raw/x.mp4", Some("  ")).is_err());
    }

    #[test]
    fn single_clip_match_emits_clip_id_not_asset_path() {
        // The matched clip's own id (here its name "clip-0", no uuid) is the
        // anchor — NOT the asset path — so apply_edl resolves by first-pass
        // name/uuid rather than the audio-counting asset fallback.
        let tmp = tempfile::tempdir().unwrap();
        write_timeline(tmp.path(), &["raw/x.mp4", "raw/other.mp4"]);
        let resolved = resolve_clip_anchor(tmp.path(), "raw/x.mp4", None).unwrap();
        assert_eq!(resolved.anchor, "clip-0");
    }

    #[test]
    fn single_clip_match_prefers_uuid_anchor() {
        // When the clip has an awidat.clip_uuid, that unique id is emitted
        // (preferred over the name) so apply_edl resolves it unambiguously.
        let tmp = tempfile::tempdir().unwrap();
        let mut clip = external_clip("Interview", "raw/x.mp4");
        clip.metadata.awidat = Some(awidat_proto::awidat_meta::AwidatClipMetadata {
            extra: [(
                "clip_uuid".to_string(),
                serde_json::Value::String("clip-abc123".to_string()),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        });
        let mut tl = Timeline::empty("test");
        tl.tracks.children.push(StackChild::Track(track_with(
            TrackKind::Video,
            "V1",
            vec![clip],
        )));
        write_otio(tmp.path(), &tl);
        let resolved = resolve_clip_anchor(tmp.path(), "raw/x.mp4", None).unwrap();
        assert_eq!(resolved.anchor, "clip-abc123");
    }

    #[test]
    fn rejects_control_chars_in_explicit_anchor() {
        let tmp = tempfile::tempdir().unwrap();
        write_timeline(tmp.path(), &["raw/x.mp4"]);
        let err = resolve_clip_anchor(tmp.path(), "raw/x.mp4", Some("clip-0\nextra")).unwrap_err();
        assert!(err.contains("control characters"), "unexpected: {err}");
    }

    #[test]
    fn two_clip_match_errors_with_count() {
        let tmp = tempfile::tempdir().unwrap();
        write_timeline(tmp.path(), &["raw/x.mp4", "raw/x.mp4"]);
        let err = resolve_clip_anchor(tmp.path(), "raw/x.mp4", None).unwrap_err();
        assert!(
            err.contains("appears in 2 clips"),
            "unexpected error: {err}"
        );
        assert!(err.contains("clip_anchor"), "unexpected error: {err}");
    }

    #[test]
    fn zero_clip_match_errors() {
        let tmp = tempfile::tempdir().unwrap();
        write_timeline(tmp.path(), &["raw/other.mp4"]);
        let err = resolve_clip_anchor(tmp.path(), "raw/x.mp4", None).unwrap_err();
        assert!(
            err.contains("no timeline clip references"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn missing_timeline_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let err = resolve_clip_anchor(tmp.path(), "raw/x.mp4", None).unwrap_err();
        assert!(err.contains("could not read timeline"), "unexpected: {err}");
    }

    // --- finding 3364897037: count only VIDEO clips ---

    /// A linked A/V pair (one video clip + one audio clip, both
    /// referencing the same media) must resolve to exactly ONE anchor —
    /// the audio half is not counted.
    #[test]
    fn linked_av_pair_resolves_to_single_video_clip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut tl = Timeline::empty("test");
        tl.tracks.children.push(StackChild::Track(track_with(
            TrackKind::Video,
            "V1",
            vec![external_clip("clip-0", "raw/x.mp4")],
        )));
        tl.tracks.children.push(StackChild::Track(track_with(
            TrackKind::Audio,
            "A1",
            vec![external_clip("clip-0-audio", "raw/x.mp4")],
        )));
        write_otio(tmp.path(), &tl);

        // Resolves to the single VIDEO clip, emitting its id ("clip-0"),
        // not the asset path — so apply_edl's first pass picks the picture
        // clip and never reaches the audio-counting asset fallback.
        let resolved = resolve_clip_anchor(tmp.path(), "raw/x.mp4", None).unwrap();
        assert_eq!(resolved.anchor, "clip-0");
    }

    // --- finding 3364897033: validate an explicit clip_anchor ---

    /// An explicit anchor that names a video clip referencing the asset
    /// resolves OK (match by clip name).
    #[test]
    fn explicit_anchor_matching_video_clip_ok() {
        let tmp = tempfile::tempdir().unwrap();
        write_timeline(tmp.path(), &["raw/x.mp4", "raw/x.mp4"]);
        let resolved = resolve_clip_anchor(tmp.path(), "raw/x.mp4", Some("clip-0")).unwrap();
        assert_eq!(resolved.anchor, "clip-0");
    }

    /// An explicit anchor that matches a clip whose media is NOT the
    /// requested asset must error.
    #[test]
    fn explicit_anchor_not_referencing_asset_errors() {
        let tmp = tempfile::tempdir().unwrap();
        write_timeline(tmp.path(), &["raw/x.mp4", "raw/other.mp4"]);
        // `clip-1` references raw/other.mp4, not raw/x.mp4. It matches the
        // anchor name but resolves to the wrong asset.
        let err = resolve_clip_anchor(tmp.path(), "raw/x.mp4", Some("clip-1")).unwrap_err();
        assert!(
            err.contains("does not match any video clip referencing")
                || err.contains("resolves to a clip referencing"),
            "unexpected error: {err}"
        );
    }

    /// Finding 3364995223: when an EARLIER video clip (timeline order)
    /// shares the anchor name/uuid but references a DIFFERENT asset,
    /// `apply_edl` resolves to that earlier (wrong-asset) clip. The planner
    /// must mirror that first-match rule: it resolves to the FIRST matching
    /// clip and, because its media is not the requested asset, errors —
    /// rather than falling through to a later clip with the right asset.
    #[test]
    fn explicit_anchor_first_match_wrong_asset_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let mut tl = Timeline::empty("test");
        // Two clips share the name "Take" but reference different assets;
        // the FIRST references the wrong asset.
        tl.tracks.children.push(StackChild::Track(track_with(
            TrackKind::Video,
            "V1",
            vec![
                external_clip("Take", "raw/other.mp4"),
                external_clip("Take", "raw/x.mp4"),
            ],
        )));
        write_otio(tmp.path(), &tl);

        let err = resolve_clip_anchor(tmp.path(), "raw/x.mp4", Some("Take")).unwrap_err();
        assert!(
            err.contains("does not match any video clip referencing")
                || err.contains("resolves to a clip referencing"),
            "unexpected error: {err}"
        );
    }

    /// An explicit anchor naming an AUDIO clip (not a video clip) must
    /// error — color anchors to the picture.
    #[test]
    fn explicit_anchor_naming_audio_clip_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let mut tl = Timeline::empty("test");
        tl.tracks.children.push(StackChild::Track(track_with(
            TrackKind::Video,
            "V1",
            vec![external_clip("vid", "raw/x.mp4")],
        )));
        tl.tracks.children.push(StackChild::Track(track_with(
            TrackKind::Audio,
            "A1",
            vec![external_clip("aud", "raw/x.mp4")],
        )));
        write_otio(tmp.path(), &tl);

        let err = resolve_clip_anchor(tmp.path(), "raw/x.mp4", Some("aud")).unwrap_err();
        // apply_edl's all-track first-match lands on the audio clip; we
        // mirror that and reject it as non-video rather than silently
        // resolving the video clip apply_edl would not pick.
        assert!(err.contains("non-video clip"), "unexpected error: {err}");
    }

    // --- finding 3364897030: return the matched clip's source range ---

    /// A trimmed clip (start>0, duration<media) yields its source range so
    /// the caller can sample within the trim.
    #[test]
    fn resolve_returns_clip_source_range() {
        let tmp = tempfile::tempdir().unwrap();
        let mut clip = external_clip("clip-0", "raw/x.mp4");
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(48.0, 24.0), // 2.0 s
            RationalTime::new(72.0, 24.0), // 3.0 s
        ));
        let mut tl = Timeline::empty("test");
        tl.tracks.children.push(StackChild::Track(track_with(
            TrackKind::Video,
            "V1",
            vec![clip],
        )));
        write_otio(tmp.path(), &tl);

        let resolved = resolve_clip_anchor(tmp.path(), "raw/x.mp4", None).unwrap();
        let (start, dur) = resolved.source_range.expect("expected a source range");
        assert!((start - 2.0).abs() < 1e-9, "start={start}");
        assert!((dur - 3.0).abs() < 1e-9, "dur={dur}");
    }

    /// A clip with no `source_range` yields `None` (whole-asset fallback).
    #[test]
    fn resolve_returns_none_range_without_source_range() {
        let tmp = tempfile::tempdir().unwrap();
        write_timeline(tmp.path(), &["raw/x.mp4"]);
        let resolved = resolve_clip_anchor(tmp.path(), "raw/x.mp4", None).unwrap();
        assert_eq!(resolved.source_range, None);
    }
}
