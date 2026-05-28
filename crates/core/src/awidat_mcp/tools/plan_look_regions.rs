//! `plan_look_regions` — generate a look-region/LUT plan from color
//! sidecars. Ported from `crates/core/src/tools/plan_look_regions.rs`
//! to the in-process MCP server.
//!
//! The original tool drives the bundled color-corrector L3 scripts.
//! This port wraps the same script invocation but resolves the script
//! path via `awidat_config::defaults::skills_root()` since the MCP
//! server context does not carry a `SkillRegistry`.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::awidat_mcp::context::McpToolCtx;

/// Arguments to `plan_look_regions`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct PlanLookRegionsArgs {
    /// Look preset. Default natural for correction/matching; use
    /// cinematic/warm/cool/punchy when the user wants a creative look.
    #[serde(default = "default_style")]
    pub style: String,
    /// Optional color-analysis sidecar paths, absolute or
    /// project-relative. Omit to use every JSON sidecar under
    /// index/color-analysis/.
    #[serde(default)]
    pub color_indexes: Vec<String>,
    /// Filename stem under renders/ for the plan artifacts. Default
    /// look-plan.
    #[serde(default = "default_plan_stem")]
    pub output_stem: String,
    /// Generated .cube LUT size. Default 17. Bounds: 2..=65.
    #[serde(default = "default_lut_size")]
    pub lut_size: u32,
    /// If true, do not emit Split Clip ops for multiple color regions
    /// in one clip.
    #[serde(default)]
    pub no_splits: bool,
}

impl Default for PlanLookRegionsArgs {
    fn default() -> Self {
        Self {
            style: default_style(),
            color_indexes: Vec::new(),
            output_stem: default_plan_stem(),
            lut_size: default_lut_size(),
            no_splits: false,
        }
    }
}

fn default_style() -> String {
    "natural".into()
}

fn default_plan_stem() -> String {
    "look-plan".into()
}

fn default_lut_size() -> u32 {
    17
}

/// Run `plan_look_regions` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`; script,
/// argument-validation, or fs failures return `Err(String)`.
pub async fn run(args: PlanLookRegionsArgs, ctx: McpToolCtx) -> Result<String, String> {
    validate_style(&args.style)?;
    if !(2..=65).contains(&args.lut_size) {
        return Err(format!(
            "plan_look_regions: lut_size must be between 2 and 65, got {}",
            args.lut_size
        ));
    }
    let output_stem = validate_stem("output_stem", &args.output_stem)?;
    let script = color_script("look_region_plan.py")?;
    let color_indexes = if args.color_indexes.is_empty() {
        discover_color_indexes(&ctx.project_root)?
    } else {
        args.color_indexes
            .iter()
            .map(|p| resolve_project_path(&ctx.project_root, p))
            .collect::<Result<Vec<_>, _>>()?
    };
    if color_indexes.is_empty() {
        return Err(format!(
            "plan_look_regions: no color sidecars found under {}. Run start_indexing with the color-analysis indexer first, or pass color_indexes explicitly.",
            ctx.project_root.join("index/color-analysis").display()
        ));
    }

    let renders = ctx.project_root.join("renders");
    tokio::fs::create_dir_all(&renders).await.map_err(|e| {
        format!(
            "plan_look_regions: failed to create {}: {e}",
            renders.display()
        )
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
        format!(
            "plan_look_regions: failed to run python3 {}: {e}",
            script.display()
        )
    })?;
    script_output("plan_look_regions", output, Some(&json_path))
}

fn validate_style(style: &str) -> Result<(), String> {
    if matches!(style, "natural" | "cinematic" | "warm" | "cool" | "punchy") {
        Ok(())
    } else {
        Err(format!(
            "plan_look_regions: unknown style {style:?}. Use natural, cinematic, warm, cool, or punchy."
        ))
    }
}

fn validate_stem(field: &str, value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
        || trimmed == "."
    {
        return Err(format!(
            "{field} must be a plain filename stem under renders/, got {value:?}"
        ));
    }
    Ok(trimmed.to_string())
}

fn color_script(script_name: &str) -> Result<PathBuf, String> {
    if let Some(root) = awidat_config::defaults::skills_root() {
        let script = root
            .join("color-corrector")
            .join("scripts")
            .join(script_name);
        if script.is_file() {
            return Ok(script);
        }
    }
    Err(format!(
        "color-corrector script {script_name} was not found. Check bundled skills installation or AWIDAT_SKILLS_ROOT."
    ))
}

fn resolve_project_path(project_root: &Path, raw: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(raw);
    let resolved = if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    };
    Ok(resolved)
}

fn discover_color_indexes(project_root: &Path) -> Result<Vec<PathBuf>, String> {
    let root = project_root.join("index/color-analysis");
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    visit_json_files(&root, &mut out)
        .map_err(|e| format!("plan_look_regions: failed to scan {}: {e}", root.display()))?;
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
) -> Result<String, String> {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        return Err(format!(
            "{tool_name}: script failed with status {}. stdout: {} stderr: {}",
            output.status,
            empty_marker(&stdout),
            empty_marker(&stderr)
        ));
    }
    if stdout.is_empty() {
        return Err(format!("{tool_name}: script succeeded but emitted no JSON"));
    }
    let mut value: serde_json::Value = serde_json::from_str(&stdout).map_err(|e| {
        format!(
            "{tool_name}: script emitted invalid JSON ({e}): {}",
            stdout.chars().take(1000).collect::<String>()
        )
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
        .map_err(|e| format!("{tool_name}: JSON serialization failed: {e}"))
}

fn empty_marker(value: &str) -> &str {
    if value.is_empty() { "<empty>" } else { value }
}

pub const DESCRIPTION: &str = "\
Create a graph-native look-region/LUT plan from the current timeline and \
color-analysis indexes. This does not edit project.otio.json directly. \
It writes renders/<stem>.edl, renders/<stem>.json, renders/<stem>.md, and \
generated .cube LUTs under luts/generated/. After this tool, call \
apply_edl with the returned edl_path, inspect vedit_diff, render the \
timeline, then call review_look_regions on the render.\
";
