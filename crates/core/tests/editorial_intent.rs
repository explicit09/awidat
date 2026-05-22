//! Integration coverage for typed editorial intent lowering.

use awidat_core::edl::intent::{
    EditorialIntent, EditorialIntentResolver, MarkerIntent, MoveClipIntent, ProfessionalIntent,
    SplitClipIntent, TrimClipIntent,
};
use awidat_core::edl::{Anchor, AnchorContext, EdlOp, ProfessionalTimelineEdit, TransitionBetween};
use awidat_proto::awidat_meta::{Anchor as AwAnchor, AwidatClipMetadata};
use awidat_proto::otio::{
    Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
    Timeline, Track, TrackChild, TrackKind,
};
use awidat_proto::professional::SourceRange;

fn timeline_with_three_clips() -> Timeline {
    let mut timeline = Timeline::empty("intent-test");
    let mut track = Track::empty("V1", TrackKind::Video);

    for (index, snippet) in ["alpha snippet", "bravo snippet", "charlie snippet"]
        .iter()
        .enumerate()
    {
        let mut clip = Clip::empty(format!("clip-{index}"));
        clip.media_reference =
            MediaReference::External(ExternalReference::new(format!("raw/{index}.mp4")));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
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

    timeline.tracks.children.push(StackChild::Track(track));
    timeline
}

fn bravo_anchor() -> Anchor {
    Anchor::TranscriptSnippet {
        text: "bravo snippet".into(),
    }
}

#[test]
fn split_trim_move_marker_and_professional_intents_lower_to_existing_edl_ops() {
    let marker = EditorialIntent::SetMarker(MarkerIntent {
        at_s: 4.0,
        label: "story turn".into(),
        category: Some("beat".into()),
    });
    let professional = EditorialIntent::Professional(ProfessionalIntent::RollEdit {
        between: TransitionBetween {
            from: Anchor::TranscriptSnippet {
                text: "alpha snippet".into(),
            },
            to: Anchor::TranscriptSnippet {
                text: "bravo snippet".into(),
            },
        },
        delta_s: 0.25,
    });

    let intent = EditorialIntent::Batch(vec![
        EditorialIntent::SplitClip(SplitClipIntent {
            anchor: bravo_anchor(),
            at_s: 2.0,
        }),
        EditorialIntent::TrimClip(TrimClipIntent {
            anchor: bravo_anchor(),
            start_s: Some(0.5),
            end_s: Some(4.0),
        }),
        EditorialIntent::MoveClip(MoveClipIntent {
            anchor: bravo_anchor(),
            to_position: 0,
            at_s: Some(1.0),
        }),
        marker,
        professional,
    ]);

    let envelope = EditorialIntentResolver::new().resolve(intent).into_edl();

    assert_eq!(
        envelope.ops,
        vec![
            EdlOp::SplitClip {
                anchor: bravo_anchor(),
                at_s: 2.0,
            },
            EdlOp::TrimClip {
                anchor: bravo_anchor(),
                start: Some(0.5),
                end: Some(4.0),
            },
            EdlOp::MoveClip {
                anchor: bravo_anchor(),
                to_position: 0,
                at_s: Some(1.0),
                snap: None,
            },
            EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::AddMarker {
                    at_s: 4.0,
                    label: "story turn".into(),
                    category: Some("beat".into()),
                },
            },
            EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::RollEdit {
                    between: TransitionBetween {
                        from: Anchor::TranscriptSnippet {
                            text: "alpha snippet".into(),
                        },
                        to: Anchor::TranscriptSnippet {
                            text: "bravo snippet".into(),
                        },
                    },
                    delta_s: 0.25,
                },
            },
        ]
    );
}

#[test]
fn resolved_editorial_command_executes_through_apply_edl_pipeline() {
    let timeline = timeline_with_three_clips();
    let command =
        EditorialIntentResolver::new().resolve(EditorialIntent::SplitClip(SplitClipIntent {
            anchor: bravo_anchor(),
            at_s: 2.0,
        }));

    let (new_timeline, outcome) = match command.execute(&timeline, &AnchorContext::empty()) {
        Ok(result) => result,
        Err(error) => panic!("intent command should execute through existing EDL apply: {error}"),
    };

    assert_eq!(outcome.applied.len(), 1);
    let StackChild::Track(track) = &new_timeline.tracks.children[0] else {
        panic!("expected a track");
    };
    assert_eq!(track.children.len(), 4);
}

#[test]
fn append_professional_intent_lowers_to_professional_timeline_edit_edl_op() {
    let intent = EditorialIntent::Professional(ProfessionalIntent::Append {
        track: "V2".into(),
        asset: "raw/broll.mp4".into(),
        range: SourceRange {
            start_s: 3.0,
            end_s: 7.0,
        },
    });

    let envelope = EditorialIntentResolver::new().resolve(intent).into_edl();

    assert_eq!(
        envelope.ops,
        vec![EdlOp::ProfessionalTimelineEdit {
            edit: ProfessionalTimelineEdit::Append {
                track: "V2".into(),
                asset: "raw/broll.mp4".into(),
                range: SourceRange {
                    start_s: 3.0,
                    end_s: 7.0,
                },
            },
        }]
    );
}
