//! Render output path preflight checks.

use std::fs::{self, OpenOptions};
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

/// Policy for validating a render output path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OutputPathPolicy {
    /// Allow an existing output file to be replaced.
    pub allow_overwrite: bool,
}

/// Output path safety preflight error.
#[derive(Debug, Error)]
pub enum OutputPathSafetyError {
    /// The path contains unsafe traversal or root components for this context.
    #[error("unsafe render output path {path}: {message}")]
    UnsafePath {
        /// Rejected path.
        path: PathBuf,
        /// Human-readable reason.
        message: String,
    },
    /// The output resolves to an input source path.
    #[error("render output {output} matches input source {input}")]
    SameAsInput {
        /// Resolved output path.
        output: PathBuf,
        /// Matching input path.
        input: PathBuf,
    },
    /// The output duplicates another planned output.
    #[error("render output {output} duplicates planned output {duplicate}")]
    DuplicateOutput {
        /// Resolved output path.
        output: PathBuf,
        /// Matching planned output path.
        duplicate: PathBuf,
    },
    /// The output exists and overwrite is not allowed.
    #[error("render output {path} already exists; choose a new path or explicitly allow overwrite")]
    WouldOverwrite {
        /// Existing output path.
        path: PathBuf,
    },
    /// The output parent cannot be written.
    #[error("render output parent {parent} is unavailable: {message}")]
    ParentUnavailable {
        /// Parent directory.
        parent: PathBuf,
        /// Human-readable reason.
        message: String,
    },
}

/// Validate an explicit render output path before starting ffmpeg.
pub fn validate_render_output_path(
    project_root: &Path,
    output_path: &Path,
    input_paths: &[PathBuf],
    planned_outputs: &[PathBuf],
    policy: OutputPathPolicy,
) -> Result<(), OutputPathSafetyError> {
    let output = resolve_render_path(project_root, output_path)?;

    for input in input_paths {
        let input = resolve_render_path(project_root, input)?;
        if same_path(&output, &input) {
            return Err(OutputPathSafetyError::SameAsInput { output, input });
        }
    }

    for planned in planned_outputs {
        let planned = resolve_render_path(project_root, planned)?;
        if same_path(&output, &planned) {
            return Err(OutputPathSafetyError::DuplicateOutput {
                output,
                duplicate: planned,
            });
        }
    }

    if output.exists() && !policy.allow_overwrite {
        return Err(OutputPathSafetyError::WouldOverwrite { path: output });
    }

    preflight_parent_writable(&output)?;
    Ok(())
}

fn resolve_render_path(project_root: &Path, path: &Path) -> Result<PathBuf, OutputPathSafetyError> {
    let mut resolved = if path.is_absolute() {
        PathBuf::new()
    } else {
        project_root.to_path_buf()
    };

    for component in path.components() {
        match component {
            Component::Prefix(_) => {
                return Err(OutputPathSafetyError::UnsafePath {
                    path: path.to_path_buf(),
                    message: "platform-specific path prefixes are not supported".into(),
                });
            }
            Component::RootDir => {
                if !path.is_absolute() {
                    return Err(OutputPathSafetyError::UnsafePath {
                        path: path.to_path_buf(),
                        message: "relative output must not contain a root component".into(),
                    });
                }
                resolved.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(OutputPathSafetyError::UnsafePath {
                    path: path.to_path_buf(),
                    message: "parent-directory segments are not allowed".into(),
                });
            }
            Component::Normal(part) => resolved.push(part),
        }
    }

    Ok(resolved)
}

fn same_path(left: &Path, right: &Path) -> bool {
    canonical_or_self(left) == canonical_or_self(right)
}

fn canonical_or_self(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn preflight_parent_writable(path: &Path) -> Result<(), OutputPathSafetyError> {
    let parent = path
        .parent()
        .ok_or_else(|| OutputPathSafetyError::ParentUnavailable {
            parent: PathBuf::from("."),
            message: "output path has no parent directory".into(),
        })?;
    let metadata =
        fs::metadata(parent).map_err(|error| OutputPathSafetyError::ParentUnavailable {
            parent: parent.to_path_buf(),
            message: error.to_string(),
        })?;
    if !metadata.is_dir() {
        return Err(OutputPathSafetyError::ParentUnavailable {
            parent: parent.to_path_buf(),
            message: "parent is not a directory".into(),
        });
    }

    let sentinel = parent.join(format!(".montage-write-preflight-{}", std::process::id()));
    let write_result = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&sentinel);
    match write_result {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(&sentinel);
            Ok(())
        }
        Err(_) if sentinel.exists() => Ok(()),
        Err(error) => Err(OutputPathSafetyError::ParentUnavailable {
            parent: parent.to_path_buf(),
            message: error.to_string(),
        }),
    }
}
