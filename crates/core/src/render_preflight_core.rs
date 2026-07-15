//! Shared render-preflight helpers.
//!
//! Shared by the legacy `ToolHandler` path and the live MCP path so
//! `render_preflight` reports the same motion-scene summary on both
//! dispatch surfaces.

/// Summarize stored MotionScenes for preflight reporting.
///
/// Stored scenes lower natively into the render plan — text layers
/// into `counts.titles`, rect/solid layers into `counts.annotations` —
/// and never into `counts.video_overlays`. Agents repeatedly read
/// `video_overlays: 0` as "my scenes were dropped", so report the
/// stored scenes explicitly with ids, anchors, and layer counts.
pub fn motion_scene_preflight_json(timeline: &montage_proto::otio::Timeline) -> serde_json::Value {
    let scenes = timeline
        .metadata
        .montage
        .as_ref()
        .map(|metadata| metadata.motion_scenes.as_slice())
        .unwrap_or_default();
    serde_json::json!({
        "count": scenes.len(),
        "scenes": scenes
            .iter()
            .map(|scene| {
                serde_json::json!({
                    "id": scene.id,
                    "start_s": scene.start_s,
                    "duration_s": scene.duration_s,
                    "layer_count": scene.layers.len(),
                })
            })
            .collect::<Vec<_>>(),
        "note": "Stored MotionScenes render natively: text layers are counted in counts.titles and rect/solid layers in counts.annotations. They never appear in counts.video_overlays, so video_overlays: 0 does NOT mean scenes were dropped.",
    })
}

#[cfg(test)]
mod tests {
    use super::motion_scene_preflight_json;

    #[test]
    fn motion_scene_preflight_json_reports_stored_scenes() {
        let mut timeline = montage_proto::otio::Timeline::empty("preflight-scenes");
        timeline.metadata.montage = Some(montage_proto::montage_meta::MontageTimelineMetadata {
            motion_scenes: vec![montage_proto::professional::MotionScene {
                id: "ai-vs-drone-mistake-comparison-manual".into(),
                start_s: 531.14,
                duration_s: 6.0,
                fps: 30.0,
                width: 1920,
                height: 1080,
                layers: vec![montage_proto::professional::MotionSceneLayer {
                    id: "heading".into(),
                    kind: montage_proto::professional::MotionSceneLayerKind::Text,
                    from_s: 0.0,
                    duration_s: 6.0,
                    z_index: 10,
                    params: Default::default(),
                }],
                rationale: None,
            }],
            ..Default::default()
        });

        let summary = motion_scene_preflight_json(&timeline);

        assert_eq!(summary["count"], 1);
        assert_eq!(
            summary["scenes"][0]["id"],
            "ai-vs-drone-mistake-comparison-manual"
        );
        assert_eq!(summary["scenes"][0]["start_s"], 531.14);
        assert_eq!(summary["scenes"][0]["layer_count"], 1);
        let note = summary["note"].as_str().unwrap_or_default();
        assert!(note.contains("video_overlays"));
    }

    #[test]
    fn motion_scene_preflight_json_handles_empty_metadata() {
        let timeline = montage_proto::otio::Timeline::empty("no-scenes");
        let summary = motion_scene_preflight_json(&timeline);
        assert_eq!(summary["count"], 0);
        assert!(summary["scenes"].as_array().is_some_and(Vec::is_empty));
    }
}
