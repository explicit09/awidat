//! Ported Montage video tools, in MCP-native form.
//!
//! Each submodule corresponds to one tool that used to live in
//! `crates/core/src/tools/`. Step 5 of the codex-harness migration
//! ports them onto `MontageMcpServer`. The original files in
//! `crates/core/src/tools/` stay live until step 7 deletes the old
//! Montage agent loop; until then, both call sites use distinct
//! copies of the logic.
//!
//! Rule of thumb when porting:
//! - Copy the pure helpers (validation, collection, rendering) into
//!   the new module. They do NOT depend on `ToolContext`.
//! - Each `pub fn` here is callable from `montage_mcp::mod`'s
//!   `#[tool(...)]` method. The method handles serde and result
//!   shaping; the helpers handle real work.
//! - Use [`crate::montage_mcp::context::McpToolCtx::resolve`] to get
//!   the project root.

pub mod analyze_sync;
pub mod apply_edl;
pub mod apply_episode_spans;
pub mod assess_continuity;
pub mod assess_edit_quality;
pub mod attempt_completion;
pub mod broll_candidates;
pub mod clip_anchor;
pub mod clip_search;
pub mod color_scopes;
pub mod create_stringout;
pub mod diagnose_project_media;
pub mod download_yt_clip;
pub mod export_package;
pub mod fetch_x_trend_context;
pub mod find_audio_asset;
pub mod find_beat;
pub mod find_black_frames;
pub mod find_broll_opportunities;
pub mod find_dead_air;
pub mod find_episode_start;
pub mod find_eye_contact;
pub mod find_false_starts;
pub mod find_filler_words;
pub mod find_generated_broll_opportunities;
pub mod find_moment;
pub mod find_speaker_oncam;
pub mod granular_timeline;
pub mod import_media;
pub mod inspect_clip;
pub mod inspect_moment;
pub mod list_assets;
pub mod list_bins;
pub mod list_episodes;
pub mod list_looks;
pub mod list_markers;
pub mod list_stringouts;
pub mod load_project_instructions;
pub mod load_skill;
pub mod local_review_package;
pub mod manage_assets;
pub mod plan_captions;
pub mod plan_color_grade;
pub mod plan_color_grade_edl;
pub mod plan_delivery_export;
pub mod plan_emphasis;
pub mod plan_generated_media;
pub mod plan_look_regions;
pub mod plan_motion_scene;
pub mod plan_multicam;
pub mod plan_reframe;
pub mod plan_scene_aware_short_form;
pub mod plan_short_form_review;
pub mod plan_sound_design;
pub mod plan_split_edit;
pub mod plan_transition;
pub mod plan_visual_support;
pub mod plan_visual_support_proposals;
pub mod podcast_apply_accepted_edits;
pub mod podcast_audio_polish;
pub mod podcast_cleanup_candidates;
pub mod podcast_edit_proposal;
pub mod podcast_editorial_review_pack;
pub mod podcast_episode_spans;
pub mod podcast_post_draft_check;
pub mod podcast_qc_report;
pub mod podcast_smooth_cut_boundaries;
pub mod podcast_story_map;
pub mod podcast_visual_polish;
pub mod poll_generated_media_job;
pub mod poll_render;
pub mod preview_cache;
pub mod proxy_media;
pub mod read_broll_recommendations;
pub mod read_index;
pub mod read_media_intelligence;
pub mod read_media_readiness;
pub mod read_understanding;
pub mod relink_media;
pub mod render_preflight;
pub mod request_user_input;
pub mod run_preview_cache_refresh;
pub mod search_broll;
pub mod shot_summary;
pub mod start_generated_media_job;
pub mod start_indexing;
pub mod start_render;
pub mod stream_remux;
pub mod transcript_pack;
pub mod transcript_search;
pub mod transition_context;
pub mod update_plan;
pub mod use_broll;
pub mod use_generated_media;
pub mod validate_transition_choice;
pub mod vedit_blame;
pub mod vedit_branch;
pub mod vedit_changed_clip_ids;
pub mod vedit_checkout;
pub mod vedit_commit;
pub mod vedit_diff;
pub mod vedit_log;
pub mod vedit_merge;
pub mod vedit_merge_preflight;
pub mod vedit_revert;
pub mod vedit_show;
pub mod vedit_tag;
pub mod verify_render;
pub mod view_episode;
pub mod view_frame;
pub mod view_program_frame;
pub mod view_timeline;
