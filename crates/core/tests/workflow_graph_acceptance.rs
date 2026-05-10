//! Graph-native acceptance coverage for bundled output workflows.
//!
//! These tests keep the workflow catalog honest: every major output path
//! must be expressible as EDL operations that apply to the OTIO graph.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use awidat_core::edl::{AnchorContext, apply, parse};
use awidat_proto::awidat_meta::{Anchor as AwAnchor, AwidatClipMetadata};
use awidat_proto::otio::{
    Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
    Timeline, Track, TrackChild, TrackKind,
};

fn base_timeline() -> Timeline {
    let mut tl = Timeline::empty("workflow acceptance");
    let mut track = Track::empty("V1", TrackKind::Video);
    for (i, snip) in ["alpha hook", "bravo setup", "charlie payoff"]
        .iter()
        .enumerate()
    {
        let mut clip = Clip::empty(format!("clip-{i}"));
        clip.media_reference =
            MediaReference::External(ExternalReference::new(format!("raw/{i}.mp4")));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(10.0 * 24.0, 24.0),
        ));
        clip.metadata = ClipMetadata {
            awidat: Some(AwidatClipMetadata {
                anchor: Some(AwAnchor {
                    transcript_snippet: Some((*snip).to_string()),
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
fn bundled_workflows_can_land_as_edit_graph_ops() {
    let cases = [
        (
            "auto-cutter",
            r#"*** Begin EDL
*** Trim Clip
@@ anchor: transcript_snippet="alpha hook"
+ start: 0.5
*** Trim Clip
@@ anchor: transcript_snippet="charlie payoff"
+ end: 8.0
*** End EDL
"#,
        ),
        (
            "podcast-editor",
            r#"*** Begin EDL
*** Set Loudness Target
+ integrated_lufs: -16
+ true_peak_db: -1
*** Set Volume
@@ anchor: transcript_snippet="bravo setup"
+ value: 0.92
*** Trim Clip
@@ anchor: transcript_snippet="bravo setup"
+ end: 9.5
*** End EDL
"#,
        ),
        (
            "pacing-optimizer",
            r#"*** Begin EDL
*** Set Speed
@@ anchor: transcript_snippet="bravo setup"
+ factor: 1.1
*** Trim Clip
@@ anchor: transcript_snippet="charlie payoff"
+ end: 9.0
*** End EDL
"#,
        ),
        (
            "podcast-episode-producer",
            r#"*** Begin EDL
*** Insert Title
+ start_s: 0
+ end_s: 3
+ text: "Cold open"
+ position: top
*** Set Package Metadata
+ platform: youtube
+ title: "Episode package"
+ description: "A publishable episode package"
+ tags: podcast,episode
*** End EDL
"#,
        ),
        (
            "viral-clip-extractor",
            r#"*** Begin EDL
*** Set Output Format
+ aspect_ratio: 9:16
+ platform: youtube_shorts
+ safe_area: mobile
*** Insert Caption
+ start_s: 0
+ end_s: 1.5
+ text: "This changed everything"
*** Set Package Metadata
+ platform: youtube_shorts
+ title: "This Changed Everything"
*** End EDL
"#,
        ),
        (
            "short-form",
            r#"*** Begin EDL
*** Set Output Format
+ aspect_ratio: 9:16
+ platform: tiktok
+ safe_area: mobile
*** Insert Caption
+ start_s: 1
+ end_s: 2.2
+ text: "watch this"
*** Set Speed
@@ anchor: transcript_snippet="alpha hook"
+ factor: 1.08
*** End EDL
"#,
        ),
        (
            "rough-cut-assembler",
            r#"*** Begin EDL
*** Insert Clip
+ asset: raw/best-take.mp4
+ track: V1
+ at_position: 1
+ start: 0
+ end: 6
+ name: best-take
*** Move Clip
@@ anchor: transcript_snippet="charlie payoff"
+ to_position: 0
*** End EDL
"#,
        ),
        (
            "meeting-highlights",
            r#"*** Begin EDL
*** Insert Title
+ start_s: 0
+ end_s: 2
+ text: "Decision"
+ position: top
*** Trim Clip
@@ anchor: transcript_snippet="bravo setup"
+ start: 1
+ end: 8
*** End EDL
"#,
        ),
        (
            "beat-sync-editor",
            r#"*** Begin EDL
*** Insert Transition
@@ between: transcript_snippet="alpha hook" and transcript_snippet="bravo setup"
+ kind: SMPTE_Dissolve
+ duration_s: 0.25
*** Set Speed
@@ anchor: transcript_snippet="charlie payoff"
+ factor: 1.2
*** End EDL
"#,
        ),
    ];

    for (workflow, edl) in cases {
        let env = parse(edl).unwrap_or_else(|err| panic!("{workflow} EDL failed parse: {err}"));
        let (_timeline, outcome) = apply(&base_timeline(), &env, &AnchorContext::empty())
            .unwrap_or_else(|err| panic!("{workflow} EDL failed apply: {err}"));
        assert_eq!(
            outcome.applied.len(),
            env.ops.len(),
            "{workflow} did not apply every graph op"
        );
    }
}
