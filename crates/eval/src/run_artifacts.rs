//! Immutable on-disk artifacts for one scenario run.

use std::path::{Path, PathBuf};

use serde_json::json;
use thiserror::Error;

use crate::Scenario;

/// Paths owned by one scenario inside one eval run.
#[derive(Debug)]
pub struct RunArtifacts {
    root: PathBuf,
}

/// Paths owned by one immutable edit attempt.
#[derive(Debug)]
pub struct AttemptArtifacts {
    root: PathBuf,
}

/// Failure to create or initialize run artifacts.
#[derive(Debug, Error)]
pub enum RunArtifactError {
    /// An identifier could escape or alias the expected run layout.
    #[error("invalid {kind}: {value:?}")]
    InvalidIdentifier {
        /// Identifier class.
        kind: &'static str,
        /// Rejected value.
        value: String,
    },
    /// A filesystem operation failed.
    #[error("{action} {}: {source}", path.display())]
    Io {
        /// Operation being attempted.
        action: &'static str,
        /// Affected path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// Input manifest serialization failed.
    #[error("serializing input manifest: {0}")]
    Json(#[from] serde_json::Error),
}

impl RunArtifacts {
    /// Create `runs/<run_id>/<scenario_id>/` and its immutable inputs.
    pub fn create(
        runs_root: impl AsRef<Path>,
        run_id: &str,
        scenario: &Scenario,
    ) -> Result<Self, RunArtifactError> {
        validate_identifier("run id", run_id)?;
        validate_identifier("scenario id", &scenario.id)?;

        let run_root = runs_root.as_ref().join(run_id);
        create_dir_all(&run_root, "creating run directory")?;
        let root = run_root.join(&scenario.id);
        create_dir(&root, "creating scenario directory")?;

        let task_path = root.join("task.md");
        write(&task_path, scenario.task.as_bytes(), "writing task")?;
        let manifest_path = root.join("input_manifest.json");
        let manifest = json!({
            "scenario_id": scenario.id,
            "category": scenario.category,
            "tool": scenario.tool,
            "source": scenario.source,
        });
        let bytes = serde_json::to_vec_pretty(&manifest)?;
        write(&manifest_path, &bytes, "writing input manifest")?;

        Ok(Self { root })
    }

    /// Scenario run root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Agent-visible task artifact.
    pub fn task_path(&self) -> PathBuf {
        self.root.join("task.md")
    }

    /// Immutable input identity artifact.
    pub fn input_manifest_path(&self) -> PathBuf {
        self.root.join("input_manifest.json")
    }

    /// Create a fresh attempt directory; an existing attempt is an error.
    pub fn create_attempt(&self, number: u32) -> Result<AttemptArtifacts, RunArtifactError> {
        if number == 0 {
            return Err(RunArtifactError::InvalidIdentifier {
                kind: "attempt number",
                value: number.to_string(),
            });
        }
        let root = self.root.join(format!("attempt_{number}"));
        create_dir(&root, "creating attempt directory")?;
        create_dir(&root.join("evidence"), "creating evidence directory")?;
        Ok(AttemptArtifacts { root })
    }
}

impl AttemptArtifacts {
    /// Attempt root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Evidence packet directory.
    pub fn evidence_dir(&self) -> PathBuf {
        self.root.join("evidence")
    }
}

fn validate_identifier(kind: &'static str, value: &str) -> Result<(), RunArtifactError> {
    let valid = !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if valid {
        Ok(())
    } else {
        Err(RunArtifactError::InvalidIdentifier {
            kind,
            value: value.to_string(),
        })
    }
}

fn create_dir(path: &Path, action: &'static str) -> Result<(), RunArtifactError> {
    std::fs::create_dir(path).map_err(|source| RunArtifactError::Io {
        action,
        path: path.to_path_buf(),
        source,
    })
}

fn create_dir_all(path: &Path, action: &'static str) -> Result<(), RunArtifactError> {
    std::fs::create_dir_all(path).map_err(|source| RunArtifactError::Io {
        action,
        path: path.to_path_buf(),
        source,
    })
}

fn write(path: &Path, bytes: &[u8], action: &'static str) -> Result<(), RunArtifactError> {
    std::fs::write(path, bytes).map_err(|source| RunArtifactError::Io {
        action,
        path: path.to_path_buf(),
        source,
    })
}
