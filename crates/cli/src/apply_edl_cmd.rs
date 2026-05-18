//! `awidat apply-edl` — apply a freeform EDL envelope to a project timeline.

use std::path::Path;

use anyhow::{Context, Result};
use awidat_core::edl::{AnchorContext, apply, parse};
use awidat_proto::project::Project;

pub fn run(project_root: &Path, edl_path: &Path) -> Result<()> {
    let edl = std::fs::read_to_string(edl_path)
        .with_context(|| format!("failed to read EDL at {}", edl_path.display()))?;
    let envelope = parse(&edl).context("failed to parse EDL")?;
    let project = Project::read(project_root)
        .with_context(|| format!("failed to read project at {}", project_root.display()))?;
    let anchor_ctx = AnchorContext::with_project_root(project_root.to_path_buf());
    let (timeline, outcome) =
        apply(&project.timeline, &envelope, &anchor_ctx).context("failed to apply EDL")?;

    let mut updated = project;
    updated.timeline = timeline;
    updated
        .write(project_root)
        .with_context(|| format!("failed to write project at {}", project_root.display()))?;
    println!("Applied {} EDL op(s):", outcome.applied.len());
    for applied in outcome.applied {
        println!("  - {}", applied.description);
    }
    Ok(())
}
