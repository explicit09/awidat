//! Render an Awidat project timeline with the shared render planner.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let Some(project_root) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: cargo run -p awidat-render --example render_timeline -- <project-root>");
        return ExitCode::from(2);
    };

    let spec = match awidat_render::build_timeline_render_spec(&project_root) {
        Ok(spec) => spec,
        Err(err) => {
            eprintln!("render plan failed: {err}");
            return ExitCode::from(1);
        }
    };
    println!("output_path={}", spec.output_path.display());
    println!(
        "total_duration_s={}",
        spec.total_duration_s.unwrap_or_default()
    );

    let ffmpeg = match awidat_render::ffmpeg_path() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("ffmpeg unavailable: {err}");
            return ExitCode::from(1);
        }
    };
    let mut command = Command::new(ffmpeg);
    command.args(&spec.args);
    if let Some(cwd) = spec.cwd {
        command.current_dir(cwd);
    }
    match command.status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(err) => {
            eprintln!("failed to start ffmpeg: {err}");
            ExitCode::from(1)
        }
    }
}
