//! Editable subtitle interchange primitives.
//!
//! This module deliberately covers only deterministic text-track basics:
//! timeline-relative cues, SRT/WebVTT parse/format, and collision
//! validation. Renderer styling and burn-in remain separate graph concerns.

use serde::{Deserialize, Serialize};
use thiserror::Error;

const EPSILON_S: f64 = 0.000_001;

/// Supported subtitle interchange formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleFormat {
    /// SubRip `.srt`.
    Srt,
    /// WebVTT `.vtt`.
    Vtt,
}

/// One timeline-relative subtitle cue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubtitleCue {
    /// Optional durable cue id.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id: Option<String>,
    /// Cue start on the edited timeline.
    pub start_s: f64,
    /// Cue end on the edited timeline.
    pub end_s: f64,
    /// Cue text. Newlines are preserved.
    pub text: String,
}

impl SubtitleCue {
    /// Build a validated cue.
    pub fn new(start_s: f64, end_s: f64, text: impl Into<String>) -> Result<Self, SubtitleError> {
        let cue = Self {
            id: None,
            start_s,
            end_s,
            text: text.into(),
        };
        validate_cue(&cue, 0)?;
        Ok(cue)
    }
}

/// Editable timeline subtitle track.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SubtitleTrack {
    /// Stable track id.
    pub id: String,
    /// Human-readable track label.
    pub label: String,
    /// Optional BCP-47-ish language tag.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub language: Option<String>,
    /// Original interchange source format, when imported from a text file.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_format: Option<SubtitleFormat>,
    /// Timeline-relative cues.
    #[serde(default)]
    pub cues: Vec<SubtitleCue>,
}

impl SubtitleTrack {
    /// Convert transcript-like segments into an editable subtitle track.
    pub fn from_transcript_segments(
        id: impl Into<String>,
        label: impl Into<String>,
        language: Option<&str>,
        segments: Vec<TranscriptSegment>,
    ) -> Result<Self, SubtitleError> {
        let cues = segments
            .into_iter()
            .map(|segment| SubtitleCue::new(segment.start_s, segment.end_s, segment.text))
            .collect::<Result<Vec<_>, _>>()?;
        let track = Self {
            id: id.into(),
            label: label.into(),
            language: language.map(str::to_string),
            source_format: None,
            cues,
        };
        track.validate()?;
        Ok(track)
    }

    /// Validate track identity, cue timing, text, order, and overlap.
    pub fn validate(&self) -> Result<(), SubtitleError> {
        if self.id.trim().is_empty() {
            return Err(SubtitleError::InvalidTrack(
                "track id must be non-empty".into(),
            ));
        }
        if self.label.trim().is_empty() {
            return Err(SubtitleError::InvalidTrack(
                "track label must be non-empty".into(),
            ));
        }
        let mut previous_end = None;
        for (index, cue) in self.cues.iter().enumerate() {
            validate_cue(cue, index)?;
            if let Some(end_s) = previous_end
                && cue.start_s < end_s - EPSILON_S
            {
                return Err(SubtitleError::InvalidCue {
                    index,
                    message: "cue overlaps previous cue".into(),
                });
            }
            previous_end = Some(cue.end_s);
        }
        Ok(())
    }
}

/// Transcript segment shape used for deterministic subtitle conversion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// Segment start in timeline or source seconds, decided by caller.
    pub start_s: f64,
    /// Segment end in the same coordinate space as `start_s`.
    pub end_s: f64,
    /// Spoken text.
    pub text: String,
}

/// Subtitle parse/validation error.
#[derive(Debug, Error, PartialEq)]
pub enum SubtitleError {
    /// The track-level contract is invalid.
    #[error("invalid subtitle track: {0}")]
    InvalidTrack(String),
    /// A cue failed validation.
    #[error("invalid subtitle cue {index}: {message}")]
    InvalidCue {
        /// Cue index.
        index: usize,
        /// Diagnostic.
        message: String,
    },
    /// Text interchange parse failed.
    #[error("subtitle parse failed: {0}")]
    Parse(String),
}

/// Parse SRT text into cues.
pub fn parse_srt(input: &str) -> Result<Vec<SubtitleCue>, SubtitleError> {
    parse_blocks(input, SubtitleFormat::Srt)
}

/// Parse WebVTT text into cues.
pub fn parse_vtt(input: &str) -> Result<Vec<SubtitleCue>, SubtitleError> {
    parse_blocks(input, SubtitleFormat::Vtt)
}

/// Format cues as SRT.
pub fn format_srt(cues: &[SubtitleCue]) -> String {
    let mut out = String::new();
    for (index, cue) in cues.iter().enumerate() {
        out.push_str(&(index + 1).to_string());
        out.push('\n');
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            format_time(cue.start_s, ','),
            format_time(cue.end_s, ','),
            sanitize_text(&cue.text)
        ));
    }
    out
}

/// Format cues as WebVTT.
pub fn format_vtt(cues: &[SubtitleCue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for cue in cues {
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            format_time(cue.start_s, '.'),
            format_time(cue.end_s, '.'),
            sanitize_text(&cue.text)
        ));
    }
    out
}

fn parse_blocks(input: &str, format: SubtitleFormat) -> Result<Vec<SubtitleCue>, SubtitleError> {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = Vec::new();
    let mut logical_blocks = normalized.split("\n\n");
    if matches!(format, SubtitleFormat::Vtt)
        && logical_blocks
            .clone()
            .next()
            .is_some_and(|block| block.trim_start().starts_with("WEBVTT"))
    {
        logical_blocks.next();
    }
    for block in logical_blocks {
        let lines = block
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>();
        if lines.is_empty() {
            continue;
        }
        if lines[0].starts_with("NOTE") || lines[0].starts_with("STYLE") {
            continue;
        }
        let timing_index = lines
            .iter()
            .position(|line| line.contains("-->"))
            .ok_or_else(|| SubtitleError::Parse("cue block is missing timing line".into()))?;
        let text_lines = &lines[(timing_index + 1)..];
        if text_lines.is_empty() {
            return Err(SubtitleError::Parse("cue block is missing text".into()));
        }
        let (start_s, end_s) = parse_timing_line(lines[timing_index])?;
        let cue = SubtitleCue {
            id: None,
            start_s,
            end_s,
            text: text_lines.join("\n"),
        };
        validate_cue(&cue, cues.len())?;
        cues.push(cue);
    }
    validate_cue_order(&cues)?;
    Ok(cues)
}

fn parse_timing_line(line: &str) -> Result<(f64, f64), SubtitleError> {
    let mut parts = line.split("-->");
    let start = parts
        .next()
        .ok_or_else(|| SubtitleError::Parse("missing cue start".into()))?
        .trim();
    let end = parts
        .next()
        .ok_or_else(|| SubtitleError::Parse("missing cue end".into()))?
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim();
    if parts.next().is_some() {
        return Err(SubtitleError::Parse(
            "cue timing has too many arrows".into(),
        ));
    }
    Ok((parse_time(start)?, parse_time(end)?))
}

fn parse_time(value: &str) -> Result<f64, SubtitleError> {
    let value = value.replace(',', ".");
    let parts = value.split(':').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(SubtitleError::Parse(format!(
            "timestamp '{value}' must be HH:MM:SS.mmm"
        )));
    }
    let hours = parts[0]
        .parse::<u64>()
        .map_err(|_| SubtitleError::Parse(format!("bad timestamp hours in '{value}'")))?;
    let minutes = parts[1]
        .parse::<u64>()
        .map_err(|_| SubtitleError::Parse(format!("bad timestamp minutes in '{value}'")))?;
    let seconds = parts[2]
        .parse::<f64>()
        .map_err(|_| SubtitleError::Parse(format!("bad timestamp seconds in '{value}'")))?;
    if minutes >= 60 || !(0.0..60.0).contains(&seconds) {
        return Err(SubtitleError::Parse(format!(
            "timestamp '{value}' has out-of-range fields"
        )));
    }
    Ok((hours * 3600 + minutes * 60) as f64 + seconds)
}

fn format_time(t: f64, sep: char) -> String {
    let t = t.max(0.0);
    let total_ms = (t * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    format!("{h:02}:{m:02}:{s:02}{sep}{ms:03}")
}

fn sanitize_text(text: &str) -> String {
    text.replace("-->", "->")
}

fn validate_cue(cue: &SubtitleCue, index: usize) -> Result<(), SubtitleError> {
    if !cue.start_s.is_finite() || cue.start_s < 0.0 {
        return Err(SubtitleError::InvalidCue {
            index,
            message: "start_s must be finite and non-negative".into(),
        });
    }
    if !cue.end_s.is_finite() || cue.end_s <= cue.start_s {
        return Err(SubtitleError::InvalidCue {
            index,
            message: "end_s must be finite and greater than start_s".into(),
        });
    }
    if cue.text.trim().is_empty() {
        return Err(SubtitleError::InvalidCue {
            index,
            message: "text must be non-empty".into(),
        });
    }
    Ok(())
}

fn validate_cue_order(cues: &[SubtitleCue]) -> Result<(), SubtitleError> {
    let track = SubtitleTrack {
        id: "parsed".into(),
        label: "Parsed subtitles".into(),
        cues: cues.to_vec(),
        ..SubtitleTrack::default()
    };
    track.validate()
}
