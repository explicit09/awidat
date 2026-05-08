//! Regression coverage for bundled workflow skills and their L3 scripts.

use std::error::Error;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use awidat_core::context::ContextualUserFragment;
use awidat_core::skills::SkillRegistry;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[test]
fn bundled_output_workflow_skills_load() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    for name in [
        "auto-cutter",
        "beat-sync-editor",
        "b-roll-suggester",
        "interview-tightener",
        "meeting-highlights",
        "pacing-optimizer",
        "podcast-editor",
        "podcast-episode-producer",
        "rough-cut-assembler",
        "short-form",
        "stock-broll",
        "tutorial",
        "version-control",
        "viral-clip-extractor",
        "yt-broll",
    ] {
        let Some(skill) = registry.get(name) else {
            panic!("missing bundled skill {name}");
        };
        assert!(
            !skill.meta.description.trim().is_empty(),
            "skill {name} must have a description"
        );
        assert!(
            !skill.body.trim().is_empty(),
            "skill {name} must have a body"
        );
    }
}

#[test]
fn l1_catalog_exposes_metadata_without_l2_or_l3_content() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    let catalog = registry
        .l1_fragment()
        .expect("bundled skills should produce an L1 fragment")
        .render();

    assert!(catalog.contains("viral-clip-extractor"));
    assert!(catalog.contains("Find and build 30-90 second social clips"));
    assert!(
        !catalog.contains("This is an awidat advantage"),
        "L1 catalog must not include SKILL.md body text"
    );
    assert!(
        !catalog.contains("scripts/score_moments.py"),
        "L1 catalog must not include L3 script paths"
    );
    assert!(
        !catalog.contains(root.to_string_lossy().as_ref()),
        "L1 catalog must not include absolute skill roots"
    );
}

#[test]
fn output_workflow_skills_keep_the_edit_graph_in_the_loop() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    for name in [
        "auto-cutter",
        "beat-sync-editor",
        "meeting-highlights",
        "pacing-optimizer",
        "podcast-editor",
        "podcast-episode-producer",
        "rough-cut-assembler",
        "short-form",
        "viral-clip-extractor",
    ] {
        let skill = registry.get(name).expect("workflow skill exists");
        assert!(
            skill
                .meta
                .tools_allowlist
                .iter()
                .any(|tool| tool == "apply_edl"),
            "{name} must expose apply_edl so analysis becomes graph edits"
        );
        assert!(
            skill
                .meta
                .tools_allowlist
                .iter()
                .any(|tool| tool == "view_timeline"),
            "{name} must expose view_timeline so the graph is inspected after edits"
        );
        assert!(
            skill
                .meta
                .tools_allowlist
                .iter()
                .any(|tool| tool == "vedit_diff"),
            "{name} must expose vedit_diff so workflow outputs have an audit checkpoint"
        );
        assert!(
            skill.body.contains("apply_edl"),
            "{name} body must tell the agent to mutate the edit graph"
        );
        assert!(
            skill.body.contains("view_timeline"),
            "{name} body must tell the agent to inspect the graph"
        );
        assert!(
            skill.body.contains("vedit_diff"),
            "{name} body must require an audit diff checkpoint"
        );
    }
}

#[test]
fn podcast_episode_skill_covers_full_editorial_process() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");
    let skill = registry
        .get("podcast-episode-producer")
        .expect("podcast skill exists");

    for tool in [
        "find_episode_start",
        "find_dead_air",
        "find_filler_words",
        "find_false_starts",
        "find_broll_opportunities",
        "apply_edl",
        "vedit_diff",
    ] {
        assert!(
            skill.meta.tools_allowlist.iter().any(|t| t == tool),
            "podcast skill must allow {tool}"
        );
    }

    for required in [
        "real ending",
        "cold open",
        "Radio edit",
        "False starts",
        "J/L-cut",
        "lower thirds",
        "Set Loudness Target",
        "Set Package Metadata",
        "derivatives",
        "Archive",
    ] {
        assert!(
            skill.body.contains(required),
            "podcast skill must cover process anchor {required:?}"
        );
    }
}

#[test]
fn skill_tool_allowlists_match_graph_native_instructions() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    for skill in registry.all() {
        let body_instructs_apply = skill.body.contains("Use `apply_edl`")
            || skill.body.contains("call `apply_edl`")
            || skill.body.contains("through `apply_edl`")
            || skill.body.contains("via `apply_edl`")
            || skill
                .body
                .contains("Hand the `edl_fragment` to `apply_edl`")
            || skill.body.contains("`apply_edl Insert Clip`");
        if body_instructs_apply {
            assert!(
                skill
                    .meta
                    .tools_allowlist
                    .iter()
                    .any(|tool| tool == "apply_edl"),
                "{} body references apply_edl but frontmatter does not allow it",
                skill.meta.name
            );
        }
        if skill.body.contains("view_timeline") {
            assert!(
                skill
                    .meta
                    .tools_allowlist
                    .iter()
                    .any(|tool| tool == "view_timeline"),
                "{} body references view_timeline but frontmatter does not allow it",
                skill.meta.name
            );
        }
        if skill.body.contains("vedit_diff") {
            assert!(
                skill
                    .meta
                    .tools_allowlist
                    .iter()
                    .any(|tool| tool == "vedit_diff"),
                "{} body references vedit_diff but frontmatter does not allow it",
                skill.meta.name
            );
        }
    }
}

#[test]
fn workflow_helper_scripts_emit_json() -> Result<(), Box<dyn Error>> {
    let Some(python) = python3()? else {
        return Ok(());
    };
    let root = workspace_root();
    let fixtures = tempfile::tempdir()?;
    let audio = write_json(
        fixtures.path().join("audio.json"),
        serde_json::json!({
            "data": {
                "duration_s": 35.0,
                "windows": [
                    {"start_s": 0.0, "rms_db": -22.0},
                    {"start_s": 5.0, "rms_db": -18.0},
                    {"start_s": 12.0, "rms_db": -45.0},
                    {"start_s": 22.0, "rms_db": -20.0}
                ],
                "silences": [{"start_s": 10.0, "end_s": 12.0}]
            }
        }),
    )?;
    let transcript = write_json(
        fixtures.path().join("transcript.json"),
        serde_json::json!({
            "data": {
                "segments": [
                    {"start_s": 0.0, "end_s": 8.0, "text": "Actually this mistake changed everything.", "speaker_id": "A"},
                    {"start_s": 8.0, "end_s": 14.0, "text": "um we decided Sarah will follow up by Friday.", "speaker_id": "A"},
                    {"start_s": 20.0, "end_s": 32.0, "text": "The biggest problem was the launch risk.", "speaker_id": "B"}
                ],
                "words": [
                    {"word": "Actually", "start_s": 0.0, "end_s": 0.4},
                    {"word": "this", "start_s": 0.4, "end_s": 0.7},
                    {"word": "mistake", "start_s": 0.7, "end_s": 1.2},
                    {"word": "changed", "start_s": 1.2, "end_s": 1.7},
                    {"word": "everything", "start_s": 1.7, "end_s": 2.2}
                ]
            }
        }),
    )?;
    let moments = write_json(
        fixtures.path().join("moments.json"),
        serde_json::json!({
            "data": {
                "moments": [
                    {"moment_id": "m1", "kind": "hook", "start_s": 0.0, "end_s": 30.0, "score": 0.9, "dependencies": [], "note": "Actually this mistake changed everything"}
                ]
            }
        }),
    )?;
    let shot = write_json(
        fixtures.path().join("shot.json"),
        serde_json::json!({
            "data": {
                "shots": [
                    {"start_s": 0.0, "end_s": 30.0, "type": "close-up", "motion": "handheld"}
                ]
            }
        }),
    )?;
    let gaze = write_json(
        fixtures.path().join("gaze.json"),
        serde_json::json!({
            "data": {
                "per_frame": [
                    {"t_s": 1.0, "faces": [{"at_camera": true}]},
                    {"t_s": 2.0, "faces": [{"at_camera": true}]}
                ]
            }
        }),
    )?;
    let quality = write_json(
        fixtures.path().join("quality.json"),
        serde_json::json!({
            "data": {
                "per_frame": [
                    {"t_s": 1.0, "is_sharp": true, "blur": 200.0, "brightness": 130.0, "contrast": 60.0},
                    {"t_s": 2.0, "is_sharp": true, "blur": 180.0, "brightness": 120.0, "contrast": 55.0}
                ]
            }
        }),
    )?;
    let topic = write_json(
        fixtures.path().join("topic.json"),
        serde_json::json!({
            "data": {
                "topics": [{"start_s": 0.0, "end_s": 35.0, "label": "Launch Risk"}]
            }
        }),
    )?;
    let beats = write_json(
        fixtures.path().join("beats.json"),
        serde_json::json!({"data": {"beats": [0.0, 0.5, 1.0, 1.5, 2.0, 2.5]}}),
    )?;
    let missing_render = fixtures.path().join("missing.mp4");

    run_script(
        &python,
        root.join("skills/auto-cutter/scripts/cleanup_plan.py"),
        &[
            "--audio-energy",
            path(&audio),
            "--transcript",
            path(&transcript),
        ],
        "cuts",
    )?;
    run_script(
        &python,
        root.join("skills/podcast-editor/scripts/audio_polish_plan.py"),
        &[
            "--audio-energy",
            path(&audio),
            "--transcript",
            path(&transcript),
        ],
        "speed_changes",
    )?;
    run_script(
        &python,
        root.join("skills/viral-clip-extractor/scripts/score_moments.py"),
        &[
            "--moments",
            path(&moments),
            "--audio-energy",
            path(&audio),
            "--transcript",
            path(&transcript),
            "--shot",
            path(&shot),
            "--gaze",
            path(&gaze),
            "--frame-quality",
            path(&quality),
            "--topic",
            path(&topic),
        ],
        "candidates",
    )?;
    run_script(
        &python,
        root.join("skills/pacing-optimizer/scripts/pacing_plan.py"),
        &[
            "--audio-energy",
            path(&audio),
            "--transcript",
            path(&transcript),
            "--topic",
            path(&topic),
            "--shot",
            path(&shot),
        ],
        "cuts",
    )?;
    run_script(
        &python,
        root.join("skills/rough-cut-assembler/scripts/score_takes.py"),
        &[
            "--audio-energy",
            path(&audio),
            "--transcript",
            path(&transcript),
            "--frame-quality",
            path(&quality),
            "--gaze",
            path(&gaze),
        ],
        "takes",
    )?;
    run_script(
        &python,
        root.join("skills/meeting-highlights/scripts/classify_meeting.py"),
        &["--transcript", path(&transcript)],
        "segments",
    )?;
    run_script(
        &python,
        root.join("skills/beat-sync-editor/scripts/beat_cut_plan.py"),
        &["--beats", path(&beats), "--shot", path(&shot)],
        "cuts",
    )?;
    run_script(
        &python,
        root.join("skills/short-form/scripts/caption_plan.py"),
        &["--transcript", path(&transcript)],
        "phrases",
    )?;
    run_script(
        &python,
        root.join("skills/podcast-episode-producer/scripts/metadata_plan.py"),
        &[
            "--transcript",
            path(&transcript),
            "--topic",
            path(&topic),
            "--moments",
            path(&moments),
            "--frame-quality",
            path(&quality),
        ],
        "title",
    )?;
    run_script(
        &python,
        root.join("skills/short-form/scripts/render_verify.py"),
        &["--file", path(&missing_render)],
        "status",
    )?;

    Ok(())
}

fn write_json(path: PathBuf, value: serde_json::Value) -> Result<PathBuf, Box<dyn Error>> {
    std::fs::write(&path, serde_json::to_vec_pretty(&value)?)?;
    Ok(path)
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap_or("")
}

fn python3() -> Result<Option<PathBuf>, Box<dyn Error>> {
    match Command::new("python3").arg("--version").output() {
        Ok(out) if out.status.success() => Ok(Some(PathBuf::from("python3"))),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Box::new(e)),
    }
}

fn run_script(
    python: &Path,
    script: PathBuf,
    args: &[&str],
    expected_key: &str,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let output = Command::new(python).arg(&script).args(args).output()?;
    assert!(
        output.status.success(),
        "script {} failed: {}",
        script.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert!(
        json.get(expected_key).is_some(),
        "script {} did not emit key {expected_key}; got {json}",
        script.display()
    );
    Ok(json)
}
