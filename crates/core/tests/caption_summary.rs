//! Integration coverage for shared caption/subtitle inventory.

use awidat_core::captions::{CaptionAuthority, caption_summary_metadata, summarize_captions};
use awidat_proto::otio::{Clip, Effect, StackChild, Track, TrackChild, TrackKind};
use awidat_proto::project::{EditPlan, Project};
use awidat_proto::subtitle::{SubtitleCue, SubtitleTrack};

fn project_with_timeline(timeline: awidat_proto::otio::Timeline) -> Project {
    Project {
        root: std::path::PathBuf::new(),
        timeline,
        edit_plan: EditPlan::empty(),
        manifest: None,
        warnings: Vec::new(),
    }
}

fn caption_clip(name: &str, word_timed: bool, safe_area: Option<&str>) -> Clip {
    let mut clip = Clip::empty(name);
    let mut effect = Effect::new("awidat.title");
    effect
        .metadata
        .insert("role".into(), serde_json::json!("caption"));
    if let Some(safe_area) = safe_area {
        effect
            .metadata
            .insert("safe_area".into(), serde_json::json!(safe_area));
    }
    if word_timed {
        effect.metadata.insert(
            "word_timings".into(),
            serde_json::json!([
                {"text": "Hello", "start_s": 0.0, "end_s": 0.2},
                {"text": "world", "start_s": 0.2, "end_s": 0.5}
            ]),
        );
    }
    clip.effects.push(effect);
    clip
}

#[test]
fn caption_summary_prefers_editable_tracks_and_counts_overlays()
-> Result<(), Box<dyn std::error::Error>> {
    let mut timeline = awidat_proto::otio::Timeline::empty("caption-summary");
    let metadata = timeline
        .metadata
        .awidat
        .as_mut()
        .ok_or("timeline metadata missing")?;
    metadata.subtitle_tracks.push(SubtitleTrack {
        id: "captions-en".into(),
        label: "English captions".into(),
        language: Some("en".into()),
        source_format: None,
        cues: vec![SubtitleCue::new(1.0, 2.0, "Edited caption")?],
    });

    let mut titles = Track::empty("Titles", TrackKind::Video);
    titles.children.push(TrackChild::Clip(caption_clip(
        "caption-a",
        true,
        Some("mobile"),
    )));
    titles
        .children
        .push(TrackChild::Clip(caption_clip("caption-b", false, None)));
    timeline.tracks.children.push(StackChild::Track(titles));

    let summary = summarize_captions(&project_with_timeline(timeline));

    assert_eq!(
        summary.selected_authority,
        CaptionAuthority::EditableSubtitleTracks
    );
    assert_eq!(summary.editable_track_count, 1);
    assert_eq!(summary.editable_cue_count, 1);
    assert_eq!(summary.caption_overlay_count, 2);
    assert_eq!(summary.word_timed_caption_overlay_count, 1);
    assert_eq!(summary.safe_area_caption_overlay_count, 1);
    assert_eq!(summary.mobile_safe_area_caption_overlay_count, 1);
    assert_eq!(summary.missing_safe_area_caption_overlay_count, 1);
    assert!(summary.has_exportable_captions);
    assert!(summary.warnings.iter().any(|warning| {
        warning.contains("caption overlays exist") && warning.contains("editable subtitle tracks")
    }));
    assert!(
        summary
            .warnings
            .iter()
            .any(|warning| { warning.contains("caption overlays missing safe_area") })
    );
    Ok(())
}

#[test]
fn caption_summary_metadata_uses_stable_render_manifest_keys() {
    let summary = awidat_core::captions::CaptionSummary {
        selected_authority: CaptionAuthority::CaptionOverlays,
        editable_track_count: 0,
        editable_cue_count: 0,
        caption_overlay_count: 2,
        word_timed_caption_overlay_count: 1,
        safe_area_caption_overlay_count: 1,
        mobile_safe_area_caption_overlay_count: 1,
        missing_safe_area_caption_overlay_count: 1,
        sidecar_cue_count: 3,
        has_exportable_captions: true,
        warnings: vec!["caption warning".into()],
    };

    let metadata = caption_summary_metadata(&summary);

    assert_eq!(metadata["caption_authority"], "caption_overlays");
    assert_eq!(metadata["caption_overlay_count"], "2");
    assert_eq!(metadata["word_timed_caption_overlay_count"], "1");
    assert_eq!(metadata["safe_area_caption_overlay_count"], "1");
    assert_eq!(metadata["mobile_safe_area_caption_overlay_count"], "1");
    assert_eq!(metadata["missing_safe_area_caption_overlay_count"], "1");
    assert_eq!(metadata["subtitle_sidecar_cue_count"], "3");
    assert_eq!(metadata["caption_warning_count"], "1");
}
