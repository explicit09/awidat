use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub(crate) enum AcceptanceFixture {
    CheckedIn(&'static str),
    External(PathBuf),
}
pub(crate) fn load_case(fixture: &AcceptanceFixture) -> Result<AcceptanceCase> {
    let path = match fixture {
        AcceptanceFixture::CheckedIn(name) => Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("acceptance")
            .join(name),
        AcceptanceFixture::External(path) => path.clone(),
    };
    load_case_from_path(&path)
}

pub(crate) fn load_case_from_path(path: &Path) -> Result<AcceptanceCase> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let case: AcceptanceCase =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    case.validate()?;
    Ok(case)
}
#[derive(Debug, Deserialize)]
pub(crate) struct AcceptanceCase {
    pub(crate) id: String,
    pub(crate) objective: String,
    #[serde(default)]
    pub(crate) scenario_type: Option<String>,
    pub(crate) project: AcceptanceProject,
    #[serde(default)]
    pub(crate) synthetic_media: Option<SyntheticMedia>,
    #[serde(default)]
    pub(crate) final_edl: Option<String>,
    #[serde(default)]
    pub(crate) edl_generator: Option<AcceptanceEdlGenerator>,
    pub(crate) expect: AcceptanceExpect,
}

impl AcceptanceCase {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            bail!("acceptance case id is empty");
        }
        if self.objective.trim().is_empty() {
            bail!("acceptance case objective is empty");
        }
        match (&self.final_edl, &self.edl_generator) {
            (Some(edl), None) if edl.trim().is_empty() => {
                bail!("acceptance case final_edl is empty");
            }
            (Some(_), None) | (None, Some(_)) => {}
            (Some(_), Some(_)) => {
                bail!("acceptance case must not specify both final_edl and edl_generator");
            }
            (None, None) => {
                bail!("acceptance case must specify final_edl or edl_generator");
            }
        }
        match (&self.synthetic_media, &self.project.source_path) {
            (Some(media), None) => {
                if media.segments.is_empty() {
                    bail!("acceptance case synthetic_media.segments is empty");
                }
                let media_duration = media.total_duration_s();
                if (media_duration - self.project.source_duration_s).abs() > 0.001 {
                    bail!(
                        "synthetic media duration {:.3}s does not match project source_duration_s {:.3}s",
                        media_duration,
                        self.project.source_duration_s
                    );
                }
            }
            (None, Some(path)) => {
                if !path.is_absolute() {
                    bail!("project.source_path must be absolute for real acceptance fixtures");
                }
                if self.project.source_duration_s <= 0.0 {
                    bail!("project.source_duration_s must be > 0 for real acceptance fixtures");
                }
            }
            (Some(_), Some(_)) => {
                bail!(
                    "acceptance case must not specify both synthetic_media and project.source_path"
                );
            }
            (None, None) => {
                bail!("acceptance case must specify synthetic_media or project.source_path")
            }
        }
        if self.expect.duration_min_s > self.expect.duration_max_s {
            bail!("duration_min_s must be <= duration_max_s");
        }
        if self.expect.max_long_silence_s <= 0.0 {
            bail!("max_long_silence_s must be > 0");
        }
        if self.expect.max_black_segment_s <= 0.0 || self.expect.black_min_duration_s <= 0.0 {
            bail!("black segment thresholds must be > 0");
        }
        validate_source_range_expectations(
            "removed_source_ranges",
            &self.expect.removed_source_ranges,
        )?;
        validate_source_range_expectations("kept_source_ranges", &self.expect.kept_source_ranges)?;
        if let Some(transcript) = &self.expect.transcript {
            transcript.validate()?;
        }
        if let Some(generator) = &self.edl_generator {
            generator.validate()?;
        }
        Ok(())
    }
}

fn validate_source_range_expectations(
    field: &str,
    ranges: &[SourceRangeExpectation],
) -> Result<()> {
    for range in ranges {
        if range.start_s < 0.0 || range.end_s <= range.start_s {
            bail!("{field} entries must satisfy 0 <= start_s < end_s");
        }
        if let Some(tolerance_s) = range.tolerance_s
            && tolerance_s < 0.0
        {
            bail!("{field} tolerance_s must be >= 0");
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub(crate) struct AcceptanceProject {
    pub(crate) name: String,
    pub(crate) source_asset: String,
    pub(crate) source_duration_s: f64,
    #[serde(default)]
    pub(crate) source_path: Option<PathBuf>,
    #[serde(default)]
    pub(crate) source_start_s: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SyntheticMedia {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) fps: u32,
    pub(crate) segments: Vec<SyntheticSegment>,
}

impl SyntheticMedia {
    pub(crate) fn total_duration_s(&self) -> f64 {
        self.segments.iter().map(|segment| segment.duration_s).sum()
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct SyntheticSegment {
    pub(crate) kind: String,
    pub(crate) duration_s: f64,
    #[serde(default)]
    pub(crate) frequency_hz: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AcceptanceEdlGenerator {
    pub(crate) command: Vec<String>,
}

impl AcceptanceEdlGenerator {
    fn validate(&self) -> Result<()> {
        if self.command.is_empty() {
            bail!("edl_generator.command must not be empty");
        }
        if self.command.iter().any(|arg| arg.trim().is_empty()) {
            bail!("edl_generator.command entries must not be empty");
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct AcceptanceExpect {
    pub(crate) duration_min_s: f64,
    pub(crate) duration_max_s: f64,
    pub(crate) max_long_silence_s: f64,
    pub(crate) silence_threshold_db: f64,
    pub(crate) max_black_segment_s: f64,
    pub(crate) black_min_duration_s: f64,
    pub(crate) min_clip_count: usize,
    #[serde(default)]
    pub(crate) edl_must_contain: Vec<String>,
    #[serde(default)]
    pub(crate) removed_source_ranges: Vec<SourceRangeExpectation>,
    #[serde(default)]
    pub(crate) kept_source_ranges: Vec<SourceRangeExpectation>,
    #[serde(default)]
    pub(crate) transcript: Option<AcceptanceTranscriptExpect>,
}
pub(crate) const DEFAULT_SOURCE_RANGE_TOLERANCE_S: f64 = 0.15;

#[derive(Debug, Deserialize)]
pub(crate) struct SourceRangeExpectation {
    pub(crate) start_s: f64,
    pub(crate) end_s: f64,
    #[serde(default)]
    pub(crate) reason: Option<String>,
    #[serde(default)]
    pub(crate) tolerance_s: Option<f64>,
}
#[derive(Debug, Deserialize)]
pub(crate) struct AcceptanceTranscriptExpect {
    pub(crate) segments: Vec<TranscriptSegment>,
    #[serde(default)]
    pub(crate) must_preserve: Vec<String>,
    #[serde(default)]
    pub(crate) must_remove: Vec<String>,
}

impl AcceptanceTranscriptExpect {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.segments.is_empty() {
            bail!("transcript.segments must not be empty when transcript checks are configured");
        }
        for segment in &self.segments {
            if segment.text.trim().is_empty() {
                bail!("transcript segment text must not be empty");
            }
            if segment.start_s < 0.0 || segment.end_s <= segment.start_s {
                bail!("transcript segments must satisfy 0 <= start_s < end_s");
            }
        }
        if self
            .must_preserve
            .iter()
            .any(|phrase| phrase.trim().is_empty())
            || self
                .must_remove
                .iter()
                .any(|phrase| phrase.trim().is_empty())
        {
            bail!("transcript phrase checks must not contain empty strings");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct TranscriptSegment {
    pub(crate) text: String,
    pub(crate) start_s: f64,
    pub(crate) end_s: f64,
}
