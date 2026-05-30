//! `plan_look_regions` and `review_look_regions` tools.
//!
//! These are first-class wrappers around the bundled color-corrector
//! L3 scripts. The graph mutation still happens through `apply_edl`;
//! these tools create the deterministic look plan, generated LUT files,
//! and rendered review artifacts that make the LUT pass auditable.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Deserialize;
use tokio::process::Command;

use crate::FunctionCallError;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Generate a look-region plan and project-local LUTs from color sidecars.
pub struct PlanLookRegionsTool;

/// Plan, apply, and start rendering a look-region/LUT pass.
pub struct StartLookRegionPassTool;

/// Build a review package for a rendered look-region pass.
pub struct ReviewLookRegionsTool;

#[derive(Debug, Deserialize)]
struct PlanLookRegionsArgs {
    #[serde(default = "default_style")]
    style: String,
    #[serde(default)]
    color_indexes: Vec<String>,
    #[serde(default = "default_plan_stem")]
    output_stem: String,
    #[serde(default = "default_lut_size")]
    lut_size: u32,
    #[serde(default)]
    no_splits: bool,
}

#[derive(Debug, Deserialize)]
struct ReviewLookRegionsArgs {
    #[serde(default = "default_plan_json")]
    look_plan: String,
    after_render: String,
    #[serde(default)]
    before_render: Option<String>,
    #[serde(default = "default_review_stem")]
    output_stem: String,
    #[serde(default = "default_max_samples")]
    max_samples: u32,
    #[serde(default = "default_tile_width")]
    tile_width: u32,
    #[serde(default = "default_low_confidence")]
    low_confidence: f64,
}

fn default_style() -> String {
    "natural".into()
}

fn default_plan_stem() -> String {
    "look-plan".into()
}

fn default_review_stem() -> String {
    "look-review".into()
}

fn default_plan_json() -> String {
    "renders/look-plan.json".into()
}

fn default_lut_size() -> u32 {
    17
}

fn default_max_samples() -> u32 {
    24
}

fn default_tile_width() -> u32 {
    320
}

fn default_low_confidence() -> f64 {
    0.5
}

#[async_trait]
impl ToolHandler for PlanLookRegionsTool {
    fn name(&self) -> &'static str {
        "plan_look_regions"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "plan_look_regions".into(),
            description: PLAN_DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "style": {
                        "type": "string",
                        "enum": ["natural", "cinematic", "warm", "cool", "punchy"],
                        "description": "Look preset. Default natural for correction/matching; use cinematic/warm/cool/punchy when the user wants a creative look."
                    },
                    "color_indexes": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional color-analysis sidecar paths, absolute or project-relative. Omit to use every JSON sidecar under index/color-analysis/."
                    },
                    "output_stem": {
                        "type": "string",
                        "description": "Filename stem under renders/ for the plan artifacts. Default look-plan."
                    },
                    "lut_size": {
                        "type": "integer",
                        "minimum": 2,
                        "maximum": 65,
                        "description": "Generated .cube LUT size. Default 17."
                    },
                    "no_splits": {
                        "type": "boolean",
                        "description": "If true, do not emit Split Clip ops for multiple color regions in one clip."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        let style = invocation
            .args
            .get("style")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("natural");
        let stem = invocation
            .args
            .get("output_stem")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("look-plan");
        vec![ApprovalKey::new(
            "plan_look_regions",
            format!("style:{style}:renders/{stem}:luts/generated"),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: PlanLookRegionsArgs =
            serde_json::from_value(if invocation.args.is_null() {
                serde_json::json!({})
            } else {
                invocation.args
            })
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "plan_look_regions: invalid args ({e}). Optional: style, color_indexes, output_stem, lut_size, no_splits."
                ))
            })?;
        validate_style(&args.style)?;
        if !(2..=65).contains(&args.lut_size) {
            return Err(FunctionCallError::RespondToModel(format!(
                "plan_look_regions: lut_size must be between 2 and 65, got {}",
                args.lut_size
            )));
        }
        let output_stem = validate_stem("output_stem", &args.output_stem)?;
        let script = color_script(&ctx, "look_region_plan.py")?;
        let color_indexes = if args.color_indexes.is_empty() {
            discover_color_indexes(&ctx.project_root)?
        } else {
            args.color_indexes
                .iter()
                .map(|p| resolve_project_path(&ctx.project_root, p))
                .collect::<Result<Vec<_>, _>>()?
        };
        if color_indexes.is_empty() {
            return Err(FunctionCallError::RespondToModel(format!(
                "plan_look_regions: no color sidecars found under {}. Run start_indexing with the color-analysis indexer first, or pass color_indexes explicitly.",
                ctx.project_root.join("index/color-analysis").display()
            )));
        }

        let renders = ctx.project_root.join("renders");
        tokio::fs::create_dir_all(&renders).await.map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "plan_look_regions: failed to create {}: {e}",
                renders.display()
            ))
        })?;
        let edl_path = renders.join(format!("{output_stem}.edl"));
        let json_path = renders.join(format!("{output_stem}.json"));
        let report_path = renders.join(format!("{output_stem}.md"));

        let mut cmd = Command::new("python3");
        cmd.arg(&script)
            .arg("--project")
            .arg(&ctx.project_root)
            .arg("--style")
            .arg(&args.style)
            .arg("--output-edl")
            .arg(&edl_path)
            .arg("--output-json")
            .arg(&json_path)
            .arg("--report-md")
            .arg(&report_path)
            .arg("--lut-size")
            .arg(args.lut_size.to_string());
        for path in &color_indexes {
            cmd.arg("--color-index").arg(path);
        }
        if args.no_splits {
            cmd.arg("--no-splits");
        }
        cmd.current_dir(&ctx.project_root);
        let output = cmd.output().await.map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "plan_look_regions: failed to run python3 {}: {e}",
                script.display()
            ))
        })?;
        script_output("plan_look_regions", output, Some(&json_path))
    }
}

#[async_trait]
impl ToolHandler for StartLookRegionPassTool {
    fn name(&self) -> &'static str {
        "start_look_region_pass"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "start_look_region_pass".into(),
            description: START_PASS_DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "style": {
                        "type": "string",
                        "enum": ["natural", "cinematic", "warm", "cool", "punchy"],
                        "description": "Look preset. Default natural for correction/matching; use cinematic/warm/cool/punchy when the user wants a creative look."
                    },
                    "color_indexes": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional color-analysis sidecar paths, absolute or project-relative. Omit to use every JSON sidecar under index/color-analysis/."
                    },
                    "output_stem": {
                        "type": "string",
                        "description": "Filename stem under renders/ for plan artifacts. Default look-plan."
                    },
                    "lut_size": {
                        "type": "integer",
                        "minimum": 2,
                        "maximum": 65,
                        "description": "Generated .cube LUT size. Default 17."
                    },
                    "no_splits": {
                        "type": "boolean",
                        "description": "If true, do not emit Split Clip ops for multiple color regions in one clip."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        let style = invocation
            .args
            .get("style")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("natural");
        let stem = invocation
            .args
            .get("output_stem")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("look-plan");
        vec![ApprovalKey::new(
            "start_look_region_pass",
            format!("style:{style}:renders/{stem}:apply-edl:start-render"),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let plan_out = PlanLookRegionsTool
            .handle(
                ToolInvocation {
                    call_id: format!("{}:plan", invocation.call_id),
                    name: "plan_look_regions".into(),
                    args: invocation.args,
                },
                ctx.clone(),
            )
            .await?;
        let plan: serde_json::Value = serde_json::from_str(&plan_out.content).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_look_region_pass: plan_look_regions returned invalid JSON ({e})"
            ))
        })?;
        let edl_path = plan
            .get("edl_path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "start_look_region_pass: plan output had no edl_path".into(),
                )
            })?;
        let edl_text = tokio::fs::read_to_string(edl_path).await.map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_look_region_pass: failed to read generated EDL {edl_path}: {e}"
            ))
        })?;

        let apply_out = crate::tools::apply_edl::ApplyEdlTool
            .handle(
                ToolInvocation {
                    call_id: format!("{}:apply", invocation.call_id),
                    name: "apply_edl".into(),
                    args: serde_json::json!({
                        "edl": edl_text,
                        "reasoning": "Applied generated look-region/LUT plan."
                    }),
                },
                ctx.clone(),
            )
            .await?;

        let render_out = crate::tools::start_render::StartRenderTool
            .handle(
                ToolInvocation {
                    call_id: format!("{}:render", invocation.call_id),
                    name: "start_render".into(),
                    args: serde_json::json!({"scope": "timeline"}),
                },
                ctx,
            )
            .await?;
        let render: serde_json::Value = serde_json::from_str(&render_out.content).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_look_region_pass: start_render returned invalid JSON ({e})"
            ))
        })?;
        let job_id = render
            .get("job_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let output_path = render
            .get("output_path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let plan_json = plan
            .get("artifact_json")
            .and_then(serde_json::Value::as_str)
            .or_else(|| plan.get("edl_path").and_then(serde_json::Value::as_str))
            .unwrap_or("renders/look-plan.json");

        let body = serde_json::json!({
            "status": "render_started",
            "plan": plan,
            "apply_edl": apply_out.content,
            "render": render,
            "render_job_id": job_id,
            "render_output_path": output_path,
            "look_plan": plan_json,
            "next_steps": [
                format!("poll_render(job_id=\"{job_id}\") until state is done"),
                format!("review_look_regions({{\"look_plan\":\"{plan_json}\",\"after_render\":\"{output_path}\"}})")
            ]
        });
        serde_json::to_string_pretty(&body)
            .map(ToolOutput::text)
            .map_err(|e| {
                FunctionCallError::Fatal(format!(
                    "start_look_region_pass: JSON serialization failed: {e}"
                ))
            })
    }
}

#[async_trait]
impl ToolHandler for ReviewLookRegionsTool {
    fn name(&self) -> &'static str {
        "review_look_regions"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "review_look_regions".into(),
            description: REVIEW_DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "look_plan": {
                        "type": "string",
                        "description": "Path to look-region plan JSON. Absolute or project-relative. Default renders/look-plan.json."
                    },
                    "after_render": {
                        "type": "string",
                        "description": "Path to the rendered timeline after applying the generated EDL. Absolute or project-relative."
                    },
                    "before_render": {
                        "type": "string",
                        "description": "Optional baseline render for before/after comparison. Absolute or project-relative."
                    },
                    "output_stem": {
                        "type": "string",
                        "description": "Filename stem under renders/ for review artifacts. Default look-review."
                    },
                    "max_samples": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 120,
                        "description": "Maximum rendered sample frames to include. Default 24."
                    },
                    "tile_width": {
                        "type": "integer",
                        "minimum": 64,
                        "maximum": 1280,
                        "description": "Contact-sheet tile width in pixels. Default 320."
                    },
                    "low_confidence": {
                        "type": "number",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "description": "Confidence threshold below which regions are flagged for review. Default 0.5."
                    }
                },
                "required": ["after_render"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        let stem = invocation
            .args
            .get("output_stem")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("look-review");
        vec![ApprovalKey::new(
            "review_look_regions",
            format!("renders/{stem}:contact-sheet"),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: ReviewLookRegionsArgs =
            serde_json::from_value(if invocation.args.is_null() {
                serde_json::json!({})
            } else {
                invocation.args
            })
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "review_look_regions: invalid args ({e}). Required: after_render. Optional: look_plan, before_render, output_stem, max_samples, tile_width, low_confidence."
                ))
            })?;
        if !(1..=120).contains(&args.max_samples) {
            return Err(FunctionCallError::RespondToModel(format!(
                "review_look_regions: max_samples must be between 1 and 120, got {}",
                args.max_samples
            )));
        }
        if !(64..=1280).contains(&args.tile_width) {
            return Err(FunctionCallError::RespondToModel(format!(
                "review_look_regions: tile_width must be between 64 and 1280, got {}",
                args.tile_width
            )));
        }
        if !(0.0..=1.0).contains(&args.low_confidence) {
            return Err(FunctionCallError::RespondToModel(format!(
                "review_look_regions: low_confidence must be between 0 and 1, got {}",
                args.low_confidence
            )));
        }
        let output_stem = validate_stem("output_stem", &args.output_stem)?;
        let script = color_script(&ctx, "look_region_review_package.py")?;
        let look_plan = resolve_project_path(&ctx.project_root, &args.look_plan)?;
        let after_render = resolve_project_path(&ctx.project_root, &args.after_render)?;
        let before_render = args
            .before_render
            .as_deref()
            .map(|p| resolve_project_path(&ctx.project_root, p))
            .transpose()?;
        ensure_file("look_plan", &look_plan)?;
        ensure_file("after_render", &after_render)?;
        if let Some(path) = &before_render {
            ensure_file("before_render", path)?;
        }

        let renders = ctx.project_root.join("renders");
        tokio::fs::create_dir_all(&renders).await.map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "review_look_regions: failed to create {}: {e}",
                renders.display()
            ))
        })?;
        let contact_sheet = renders.join(format!("{output_stem}.ppm"));
        let report = renders.join(format!("{output_stem}.md"));
        let package_json = renders.join(format!("{output_stem}.json"));

        let mut cmd = Command::new("python3");
        cmd.arg(&script)
            .arg("--look-plan")
            .arg(&look_plan)
            .arg("--after-render")
            .arg(&after_render)
            .arg("--contact-sheet")
            .arg(&contact_sheet)
            .arg("--report-md")
            .arg(&report)
            .arg("--package-json")
            .arg(&package_json)
            .arg("--max-samples")
            .arg(args.max_samples.to_string())
            .arg("--tile-width")
            .arg(args.tile_width.to_string())
            .arg("--low-confidence")
            .arg(args.low_confidence.to_string());
        if let Some(path) = &before_render {
            cmd.arg("--before-render").arg(path);
        }
        cmd.current_dir(&ctx.project_root);
        let output = cmd.output().await.map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "review_look_regions: failed to run python3 {}: {e}",
                script.display()
            ))
        })?;
        script_output("review_look_regions", output, Some(&package_json))
    }
}

fn validate_style(style: &str) -> Result<(), FunctionCallError> {
    if matches!(style, "natural" | "cinematic" | "warm" | "cool" | "punchy") {
        Ok(())
    } else {
        Err(FunctionCallError::RespondToModel(format!(
            "plan_look_regions: unknown style {style:?}. Use natural, cinematic, warm, cool, or punchy."
        )))
    }
}

fn validate_stem(field: &str, value: &str) -> Result<String, FunctionCallError> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
        || trimmed == "."
    {
        return Err(FunctionCallError::RespondToModel(format!(
            "{field} must be a plain filename stem under renders/, got {value:?}"
        )));
    }
    Ok(trimmed.to_string())
}

fn color_script(ctx: &ToolContext, script_name: &str) -> Result<PathBuf, FunctionCallError> {
    if let Some(skill) = ctx.skills.get("color-corrector") {
        let script = skill.root.join("scripts").join(script_name);
        if script.is_file() {
            return Ok(script);
        }
    }
    if let Some(root) = awidat_config::defaults::skills_root() {
        let script = root
            .join("color-corrector")
            .join("scripts")
            .join(script_name);
        if script.is_file() {
            return Ok(script);
        }
    }
    Err(FunctionCallError::RespondToModel(format!(
        "color-corrector script {script_name} was not found. Check bundled skills installation or AWIDAT_SKILLS_ROOT."
    )))
}

fn resolve_project_path(project_root: &Path, raw: &str) -> Result<PathBuf, FunctionCallError> {
    let path = PathBuf::from(raw);
    let resolved = if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    };
    Ok(resolved)
}

fn ensure_file(label: &str, path: &Path) -> Result<(), FunctionCallError> {
    if path.is_file() {
        Ok(())
    } else {
        Err(FunctionCallError::RespondToModel(format!(
            "review_look_regions: {label} not found: {}",
            path.display()
        )))
    }
}

fn discover_color_indexes(project_root: &Path) -> Result<Vec<PathBuf>, FunctionCallError> {
    let root = project_root.join("index/color-analysis");
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    visit_json_files(&root, &mut out).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "plan_look_regions: failed to scan {}: {e}",
            root.display()
        ))
    })?;
    out.sort();
    Ok(out)
}

fn visit_json_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            visit_json_files(&path, out)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            out.push(path);
        }
    }
    Ok(())
}

fn script_output(
    tool_name: &str,
    output: std::process::Output,
    artifact_json: Option<&Path>,
) -> Result<ToolOutput, FunctionCallError> {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        return Err(FunctionCallError::RespondToModel(format!(
            "{tool_name}: script failed with status {}. stdout: {} stderr: {}",
            output.status,
            empty_marker(&stdout),
            empty_marker(&stderr)
        )));
    }
    if stdout.is_empty() {
        return Err(FunctionCallError::RespondToModel(format!(
            "{tool_name}: script succeeded but emitted no JSON"
        )));
    }
    let mut value: serde_json::Value = serde_json::from_str(&stdout).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "{tool_name}: script emitted invalid JSON ({e}): {}",
            stdout.chars().take(1000).collect::<String>()
        ))
    })?;
    if let Some(path) = artifact_json
        && let serde_json::Value::Object(obj) = &mut value
    {
        obj.insert(
            "artifact_json".into(),
            serde_json::Value::String(path.display().to_string()),
        );
    }
    serde_json::to_string_pretty(&value)
        .map(ToolOutput::text)
        .map_err(|e| {
            FunctionCallError::Fatal(format!("{tool_name}: JSON serialization failed: {e}"))
        })
}

fn empty_marker(value: &str) -> &str {
    if value.is_empty() { "<empty>" } else { value }
}

const PLAN_DESCRIPTION: &str = "\
Create a graph-native look-region/LUT plan from the current timeline and \
color-analysis indexes. This does not edit project.otio.json directly. \
It writes renders/<stem>.edl, renders/<stem>.json, renders/<stem>.md, and \
generated .cube LUTs under luts/generated/. After this tool, call \
apply_edl with the returned edl_path, inspect vedit_diff, render the \
timeline, then call review_look_regions on the render.";

const START_PASS_DESCRIPTION: &str = "\
Start the end-to-end look-region/LUT pass: plan look regions from \
color-analysis sidecars, generate project-local LUTs, apply the generated \
EDL through apply_edl, and start a timeline render. Returns the render \
job_id and the exact review_look_regions call to run after poll_render \
reports done. Use this when the user wants the agent to execute the LUT \
pipeline rather than only draft a plan.";

const REVIEW_DESCRIPTION: &str = "\
Create verification artifacts for a rendered look-region/LUT pass. Reads \
the look plan JSON and actual timeline render, samples the render at the \
plan's timeline sample times, writes a contact sheet and Markdown/JSON \
review package under renders/. Use after apply_edl + start_render/poll_render.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::SandboxMode;
    use tokio::sync::broadcast;

    fn ctx_at(root: &Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        let bundled = awidat_config::defaults::skills_root();
        let (skills, _errors) = crate::skills::SkillRegistry::discover(bundled.as_deref(), None);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(skills),
            subagent_return: None,
        }
    }

    fn invoke(name: &str, args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: name.into(),
            args,
        }
    }

    fn write_fixture_project(root: &Path) {
        std::fs::create_dir_all(root.join("index/color-analysis/raw")).unwrap();
        std::fs::write(
            root.join("project.otio.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "OTIO_SCHEMA": "Timeline.1",
                "name": "Look fixture",
                "tracks": {
                    "OTIO_SCHEMA": "Stack.1",
                    "name": "tracks",
                    "children": [{
                        "OTIO_SCHEMA": "Track.1",
                        "name": "Video 1",
                        "kind": "Video",
                        "children": [{
                            "OTIO_SCHEMA": "Clip.2",
                            "name": "main",
                            "metadata": {"awidat": {"clip_uuid": "clip-main"}},
                            "media_reference": {
                                "OTIO_SCHEMA": "ExternalReference.1",
                                "target_url": "raw/source.mp4"
                            },
                            "source_range": {
                                "start_time": {"value": 0, "rate": 1},
                                "duration": {"value": 4, "rate": 1}
                            }
                        }]
                    }]
                }
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("edit-plan.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": "0.1",
                "items": []
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("index/color-analysis/raw/source.mp4.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "asset_id": "raw/source.mp4",
                "data": {
                    "summary": {
                        "issue_tags": ["underexposed"],
                        "policy": {"recommended_action": "auto_correct", "confidence": 0.9},
                        "recommended_correction": {"exposure_ev": 0.4}
                    },
                    "scenes": [{
                        "scene_id": "scene-a",
                        "start_s": 0.0,
                        "end_s": 4.0,
                        "issue_tags": ["underexposed"],
                        "policy": {"recommended_action": "auto_correct", "confidence": 0.9},
                        "recommended_correction": {"exposure_ev": 0.4}
                    }]
                }
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn plan_auto_discovers_color_sidecars_and_writes_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project(dir.path());

        let out = PlanLookRegionsTool
            .handle(
                invoke(
                    "plan_look_regions",
                    serde_json::json!({"style": "cinematic", "output_stem": "tool-look", "lut_size": 5}),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(body["status"], "planned");
        assert_eq!(body["regions"].as_array().unwrap().len(), 1);
        assert!(dir.path().join("renders/tool-look.edl").is_file());
        assert!(dir.path().join("renders/tool-look.json").is_file());
        assert!(dir.path().join("renders/tool-look.md").is_file());
        // Stage 5 redirected generated LUTs to a content-addressed
        // cache. The previous per-look `luts/generated/` directory
        // is gone; we assert the cache directory exists and holds
        // at least one .cube instead.
        assert!(dir.path().join("luts/cache").is_dir());
        let cache_entries: Vec<_> = std::fs::read_dir(dir.path().join("luts/cache"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("cube"))
            .collect();
        assert!(
            !cache_entries.is_empty(),
            "expected at least one cached .cube, got {cache_entries:?}",
        );
        let edl_text = std::fs::read_to_string(dir.path().join("renders/tool-look.edl")).unwrap();
        let env = crate::edl::parser::parse(&edl_text).unwrap();
        assert!(env.ops.iter().any(|op| {
            matches!(
                op,
                crate::edl::op::EdlOp::ApplyLut {
                    interpolation: Some(_),
                    ..
                }
            )
        }));
    }

    #[tokio::test]
    async fn plan_multi_region_edl_splits_remaining_piece() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project(dir.path());
        std::fs::write(
            dir.path().join("index/color-analysis/raw/source.mp4.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "asset_id": "raw/source.mp4",
                "data": {
                    "summary": {
                        "issue_tags": ["underexposed", "low_contrast"],
                        "policy": {"recommended_action": "auto_correct", "confidence": 0.9},
                        "recommended_correction": {"exposure_ev": 0.4, "contrast": 1.15}
                    },
                    "scenes": [
                        {
                            "scene_id": "scene-a",
                            "start_s": 0.0,
                            "end_s": 1.0,
                            "issue_tags": ["underexposed", "low_contrast"],
                            "policy": {"recommended_action": "auto_correct", "confidence": 0.9},
                            "recommended_correction": {"exposure_ev": 0.4, "contrast": 1.15}
                        },
                        {
                            "scene_id": "scene-b",
                            "start_s": 1.0,
                            "end_s": 2.5,
                            "issue_tags": ["underexposed"],
                            "policy": {"recommended_action": "auto_correct", "confidence": 0.9},
                            "recommended_correction": {"exposure_ev": 0.35}
                        },
                        {
                            "scene_id": "scene-c",
                            "start_s": 2.5,
                            "end_s": 4.0,
                            "issue_tags": ["underexposed", "low_contrast"],
                            "policy": {"recommended_action": "auto_correct", "confidence": 0.9},
                            "recommended_correction": {"exposure_ev": 0.3, "contrast": 1.12}
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let out = PlanLookRegionsTool
            .handle(
                invoke(
                    "plan_look_regions",
                    serde_json::json!({"style": "punchy", "output_stem": "multi-look", "lut_size": 5}),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(body["regions"].as_array().unwrap().len(), 3);

        let edl_text = std::fs::read_to_string(dir.path().join("renders/multi-look.edl")).unwrap();
        assert!(edl_text.contains("@@ anchor: clip_uuid=clip-main\n+ at_s: 1"));
        assert!(edl_text.contains("@@ anchor: clip_uuid=main-b\n+ at_s: 2.5"));

        crate::tools::apply_edl::ApplyEdlTool
            .handle(
                invoke(
                    "apply_edl",
                    serde_json::json!({
                        "edl": edl_text,
                        "dry_run": true,
                        "reasoning": "Validate generated multi-region look EDL."
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn plan_reports_missing_color_indexes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("project.otio.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "OTIO_SCHEMA": "Timeline.1",
                "tracks": {"OTIO_SCHEMA": "Stack.1", "children": []}
            }))
            .unwrap(),
        )
        .unwrap();
        let err = PlanLookRegionsTool
            .handle(
                invoke("plan_look_regions", serde_json::json!({})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("no color sidecars found"),
            "got {err:?}"
        );
    }

    #[test]
    fn review_schema_requires_after_render() {
        let schema = ReviewLookRegionsTool.schema();
        assert_eq!(schema.name, "review_look_regions");
        assert!(schema.input_schema.to_string().contains("\"after_render\""));
    }

    #[tokio::test]
    async fn review_writes_contact_sheet_and_package_when_render_exists() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("renders")).unwrap();
        std::fs::write(
            dir.path().join("renders/look-plan.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1,
                "status": "planned",
                "regions": [{
                    "clip_name": "main",
                    "clip_anchor": "clip-main",
                    "asset_id": "raw/source.mp4",
                    "scene_id": "scene-a",
                    "source_start_s": 0.0,
                    "source_end_s": 2.0,
                    "timeline_start_s": 0.0,
                    "timeline_end_s": 2.0,
                    "sample_times_s": [0.1, 1.0, 1.9],
                    "source_sample_times_s": [0.1, 1.0, 1.9],
                    "issue_tags": ["underexposed"],
                    "policy": {"recommended_action": "auto_correct", "confidence": 0.9},
                    "correction": {"exposure_ev": 0.4},
                    "consistency_group": "raw-source.mp4:shadow_lift:underexposed",
                    "look_id": "shadow_lift",
                    "lut_path": "luts/generated/cinematic-shadow_lift.cube",
                    "score": 0.76,
                    "rationale": "fixture"
                }],
                "generated_luts": ["luts/generated/cinematic-shadow_lift.cube"]
            }))
            .unwrap(),
        )
        .unwrap();
        let render = dir.path().join("renders/after.mp4");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=duration=2:size=160x90:rate=12",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&render)
            .status()
            .unwrap();
        if !status.success() {
            return;
        }

        let out = ReviewLookRegionsTool
            .handle(
                invoke(
                    "review_look_regions",
                    serde_json::json!({"after_render": "renders/after.mp4", "tile_width": 80}),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(body["status"], "passed");
        assert!(dir.path().join("renders/look-review.ppm").is_file());
        assert!(dir.path().join("renders/look-review.md").is_file());
        assert!(dir.path().join("renders/look-review.json").is_file());
    }

    #[tokio::test]
    async fn review_flags_excessive_lift_when_before_render_is_available() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("renders")).unwrap();
        std::fs::write(
            dir.path().join("renders/look-plan.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1,
                "status": "planned",
                "regions": [{
                    "clip_name": "main",
                    "clip_anchor": "clip-main",
                    "asset_id": "raw/source.mp4",
                    "scene_id": "scene-a",
                    "source_start_s": 0.0,
                    "source_end_s": 2.0,
                    "timeline_start_s": 0.0,
                    "timeline_end_s": 2.0,
                    "sample_times_s": [0.1, 1.0, 1.9],
                    "source_sample_times_s": [0.1, 1.0, 1.9],
                    "issue_tags": ["underexposed"],
                    "policy": {"recommended_action": "auto_correct", "confidence": 0.9},
                    "correction": {"exposure_ev": 1.0},
                    "consistency_group": "raw-source.mp4:shadow_lift:underexposed",
                    "look_id": "shadow_lift",
                    "lut_path": "luts/generated/punchy-shadow_lift.cube",
                    "score": 0.76,
                    "rationale": "fixture"
                }],
                "generated_luts": ["luts/generated/punchy-shadow_lift.cube"]
            }))
            .unwrap(),
        )
        .unwrap();
        let before = dir.path().join("renders/before.mp4");
        let after = dir.path().join("renders/after.mp4");
        for (path, color) in [(&before, "0x202020"), (&after, "0x999999")] {
            let status = std::process::Command::new("ffmpeg")
                .args([
                    "-nostdin",
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("color=c={color}:s=160x90:d=2:r=12"),
                    "-pix_fmt",
                    "yuv420p",
                    "-y",
                ])
                .arg(path)
                .status()
                .unwrap();
            if !status.success() {
                return;
            }
        }

        let out = ReviewLookRegionsTool
            .handle(
                invoke(
                    "review_look_regions",
                    serde_json::json!({
                        "before_render": "renders/before.mp4",
                        "after_render": "renders/after.mp4",
                        "tile_width": 80
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(body["status"], "needs_review");
        assert!(
            body["visual_quality"]["flags"]
                .as_array()
                .is_some_and(|flags| flags.iter().any(|flag| flag["issue"] == "excessive_lift"))
        );
        assert_eq!(
            body["summary"]["visual_quality_flagged_regions"],
            serde_json::json!([0])
        );
    }

    #[tokio::test]
    async fn start_pass_plans_applies_and_starts_timeline_render() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project(dir.path());
        std::fs::create_dir_all(dir.path().join("raw")).unwrap();
        let raw = dir.path().join("raw/source.mp4");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=duration=4:size=160x90:rate=12",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=4",
                "-shortest",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-y",
            ])
            .arg(&raw)
            .status()
            .unwrap();
        if !status.success() {
            return;
        }

        let ctx = ctx_at(dir.path());
        let out = StartLookRegionPassTool
            .handle(
                invoke(
                    "start_look_region_pass",
                    serde_json::json!({"style": "cinematic", "lut_size": 5}),
                ),
                ctx.clone(),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(body["status"], "render_started");
        assert!(
            body["render_job_id"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        );
        assert!(dir.path().join("renders/look-plan.edl").is_file());
        assert!(dir.path().join("renders/look-plan.json").is_file());

        let project = awidat_proto::project::Project::read(dir.path()).unwrap();
        let mut saw_lut = false;
        for stack_child in &project.timeline.tracks.children {
            if let awidat_proto::otio::StackChild::Track(track) = stack_child {
                for child in &track.children {
                    if let awidat_proto::otio::TrackChild::Clip(clip) = child {
                        saw_lut |= clip
                            .effects
                            .iter()
                            .any(|e| e.effect_name == awidat_effects::LUT);
                    }
                }
            }
        }
        assert!(saw_lut, "start pass should apply generated LUT effect");

        let job_id = awidat_render::JobId(
            body["render_job_id"]
                .as_str()
                .expect("render job id")
                .to_string(),
        );
        let mut terminal = None;
        for _ in 0..100 {
            let status = ctx.job_manager.status(&job_id).await.unwrap();
            if matches!(
                status.state,
                awidat_render::JobState::Done
                    | awidat_render::JobState::Failed
                    | awidat_render::JobState::Cancelled
            ) {
                terminal = Some(status);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let terminal = terminal.expect("render should reach a terminal state");
        assert_eq!(terminal.state, awidat_render::JobState::Done);

        let review = ReviewLookRegionsTool
            .handle(
                invoke(
                    "review_look_regions",
                    serde_json::json!({
                        "look_plan": "renders/look-plan.json",
                        "after_render": terminal.output_path.display().to_string(),
                        "tile_width": 80
                    }),
                ),
                ctx,
            )
            .await
            .unwrap();
        let review_body: serde_json::Value = serde_json::from_str(&review.content).unwrap();
        assert_eq!(review_body["status"], "passed");
        assert!(dir.path().join("renders/look-review.ppm").is_file());
    }
}
