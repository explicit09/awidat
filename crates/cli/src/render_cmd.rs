//! `awidat render` — synchronous timeline export for non-interactive runs.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use awidat_render::RenderJobSpec;

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
    let manifest_path = write_cli_render_manifest(project_root, &spec)?;

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
    awidat_render::finalize_render_manifest_file(&manifest_path)
        .with_context(|| format!("finalize render manifest {}", manifest_path.display()))?;
    println!("Render complete: {}", spec.output_path.display());
    println!("Render manifest: {}", manifest_path.display());
    Ok(())
}

fn write_cli_render_manifest(
    project_root: &Path,
    spec: &RenderJobSpec,
) -> Result<std::path::PathBuf> {
    let project_path = project_root.join("project.otio.json");
    let project_hash = if project_path.is_file() {
        Some(
            awidat_render::fingerprint_file(&project_path, true)
                .with_context(|| format!("fingerprint {}", project_path.display()))?
                .sha256,
        )
    } else {
        None
    };
    let ffmpeg = awidat_render::ffmpeg_path().context("failed to locate ffmpeg for manifest")?;
    let mut argv = vec![ffmpeg.to_string_lossy().into_owned()];
    argv.extend(spec.args.iter().cloned());
    let manifest = awidat_render::planned_at_now(awidat_render::RenderExecutionManifestInput {
        created_at: String::new(),
        awidat_version: env!("CARGO_PKG_VERSION").into(),
        project_root: project_root.to_string_lossy().into_owned(),
        project_hash: project_hash.clone(),
        timeline_hash: project_hash,
        backend: awidat_render::RenderBackendKind::TimelineFfmpegReencode,
        replay: awidat_render::RenderReplayPlan::FfmpegArgv {
            argv,
            cwd: spec
                .cwd
                .as_ref()
                .map(|cwd| cwd.to_string_lossy().into_owned()),
        },
        inputs: fingerprint_manifest_inputs(project_root, &spec.input_paths)?,
        outputs: vec![awidat_render::output_artifact(&spec.output_path, true)],
        sidecars: Vec::new(),
        limitations: spec
            .limitations
            .iter()
            .map(|limitation| {
                awidat_render::limitation(limitation.kind.clone(), limitation.message.clone())
            })
            .collect(),
        verification: None,
        metadata: BTreeMap::from([("render_driver".into(), "cli_render".into())]),
    });
    let manifest_path = awidat_render::manifest_path_for_output(&spec.output_path);
    awidat_render::write_render_manifest(&manifest_path, &manifest)
        .with_context(|| format!("write render manifest {}", manifest_path.display()))?;
    Ok(manifest_path)
}

fn fingerprint_manifest_inputs(
    project_root: &Path,
    input_paths: &[std::path::PathBuf],
) -> Result<Vec<awidat_render::RenderInputFingerprint>> {
    input_paths
        .iter()
        .map(|path| {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                project_root.join(path)
            };
            awidat_render::fingerprint_file(&path, true)
                .with_context(|| format!("fingerprint input {}", path.display()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_render_manifest_records_timeline_backend() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("project.otio.json"), b"{}").unwrap();
        let output_path = dir.path().join("renders/timeline.mp4");
        let spec = RenderJobSpec {
            args: vec![
                "-i".into(),
                "raw/x.mp4".into(),
                output_path.to_string_lossy().into_owned(),
            ],
            total_duration_s: Some(1.0),
            cwd: Some(dir.path().to_path_buf()),
            output_path: output_path.clone(),
            input_paths: Vec::new(),
            manifest_path: None,
            limitations: Vec::new(),
        };

        let manifest_path = write_cli_render_manifest(dir.path(), &spec).unwrap();

        assert_eq!(
            manifest_path,
            awidat_render::manifest_path_for_output(&output_path)
        );
        let manifest = awidat_render::read_render_manifest(&manifest_path).unwrap();
        assert_eq!(
            manifest.backend,
            awidat_render::RenderBackendKind::TimelineFfmpegReencode
        );
        assert_eq!(manifest.metadata["render_driver"], "cli_render");
    }
}
