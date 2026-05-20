//! Built-in tool implementations.
//!
//! Week 3 ships [`bash`]. Week 4 adds the editorial tools per `PLAN.md`
//! §6.

pub mod analyze_sync;
pub mod apply_edl;
pub mod assess_continuity;
pub mod assess_edit_quality;
pub mod attempt_completion;
pub mod bash;
pub mod broll_candidates;
pub mod clip_search;
pub mod delegate;
pub mod delegate_all;
pub mod diagnose_project_media;
pub mod download_yt_clip;
pub mod export_package;
pub mod find_beat;
pub mod find_black_frames;
pub mod find_broll_opportunities;
pub mod find_dead_air;
pub mod find_episode_start;
pub mod find_eye_contact;
pub mod find_false_starts;
pub mod find_filler_words;
pub mod find_moment;
pub mod find_speaker_oncam;
pub mod inspect_clip;
pub mod inspect_moment;
pub mod list_assets;
pub mod list_looks;
pub mod load_skill;
pub mod plan_look_regions;
pub mod plan_multicam;
pub mod plan_transition;
pub mod poll_render;
pub mod read_index;
pub mod request_user_input;
pub mod search_broll;
pub mod shot_summary;
pub mod start_indexing;
pub mod start_render;
pub mod transition_context;
pub mod update_plan;
pub mod validate_transition_choice;
pub mod use_broll;
pub mod vedit_commit;
pub mod vedit_diff;
pub mod vedit_log;
pub mod vedit_revert;
pub mod view_episode;
pub mod view_frame;
pub mod view_timeline;
