//! `awidat render` — synchronous timeline export for non-interactive runs.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

pub fn run(project_root: &Path) -> Result<()> {
    let spec = awidat_render::build_timeline_render_spec(project_root).with_context(|| {
        format!(
            "failed to plan timeline render for {}",
            project_root.display()
        )
    })?;
    let ffmpeg = awidat_render::ffmpeg_path().context("failed to locate ffmpeg")?;
    println!(
        "Rendering timeline ({:.2}s) → {}",
        spec.total_duration_s.unwrap_or_default(),
        spec.output_path.display()
    );
    for limitation in &spec.limitations {
        println!("  ! {}: {}", limitation.kind, limitation.message);
    }

    let mut cmd = Command::new(ffmpeg);
    cmd.args(&spec.args);
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    let output = cmd.output().context("failed to spawn ffmpeg")?;
    if !output.status.success() {
        bail!(
            "ffmpeg render failed with status {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    println!("Render complete: {}", spec.output_path.display());
    Ok(())
}
