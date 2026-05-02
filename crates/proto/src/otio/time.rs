//! OTIO time value types: [`RationalTime`] and [`TimeRange`].
//!
//! These are the two value types every OTIO consumer ends up touching, so we
//! keep them simple and well-tested. `RationalTime.value` is `f64` per the
//! OTIO spec.

use serde::{Deserialize, Serialize};

use crate::ProtoError;
use crate::error::JsonPath;

/// A time value at a given rate. Mirrors `opentime.RationalTime` from
/// OpenTimelineIO 1.x.
///
/// The OTIO C++/Python implementation stores `value` as `double`; we follow
/// that. Rate is "samples per second" and must be strictly positive.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RationalTime {
    /// Number of samples at [`Self::rate`].
    pub value: f64,
    /// Samples per second. Must be `> 0.0`.
    pub rate: f64,
}

impl RationalTime {
    /// Construct a new [`RationalTime`].
    pub fn new(value: f64, rate: f64) -> Self {
        Self { value, rate }
    }

    /// Time at value 0 with the given rate.
    pub fn zero(rate: f64) -> Self {
        Self { value: 0.0, rate }
    }

    /// Convert to seconds (`value / rate`). Returns NaN if rate is zero —
    /// validation should catch zero rate before this is called.
    pub fn to_seconds(self) -> f64 {
        self.value / self.rate
    }

    /// Validate this value: rate must be positive and finite, value must be
    /// finite.
    pub(crate) fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        if !self.rate.is_finite() {
            return Err(ProtoError::validation(
                file,
                path.field("rate"),
                format!("rate must be finite, got {}", self.rate),
            ));
        }
        if self.rate <= 0.0 {
            return Err(ProtoError::validation(
                file,
                path.field("rate"),
                format!("rate must be positive, got {}", self.rate),
            ));
        }
        if !self.value.is_finite() {
            return Err(ProtoError::validation(
                file,
                path.field("value"),
                format!("value must be finite, got {}", self.value),
            ));
        }
        Ok(())
    }
}

/// A range of time. Mirrors `opentime.TimeRange` from OpenTimelineIO 1.x.
///
/// Per the OTIO spec, `duration` is non-negative.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimeRange {
    /// Start of the range.
    pub start_time: RationalTime,
    /// Length of the range. Must be `>= 0` per the OTIO spec.
    pub duration: RationalTime,
}

impl TimeRange {
    /// Construct a new [`TimeRange`].
    pub fn new(start_time: RationalTime, duration: RationalTime) -> Self {
        Self {
            start_time,
            duration,
        }
    }

    /// Validate this range: both children valid, and duration non-negative.
    pub(crate) fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        self.start_time.validate(file, &path.field("start_time"))?;
        self.duration.validate(file, &path.field("duration"))?;
        if self.duration.value < 0.0 {
            return Err(ProtoError::validation(
                file,
                path.field("duration").field("value"),
                format!("duration must be non-negative, got {}", self.duration.value),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_time_to_seconds() {
        let t = RationalTime::new(48000.0, 24000.0);
        assert!((t.to_seconds() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn rational_time_validates_positive_rate() {
        let bad = RationalTime::new(0.0, 0.0);
        let err = bad
            .validate("p.json", &JsonPath::root())
            .expect_err("zero rate should fail");
        assert!(err.to_string().contains("rate must be positive"));
    }

    #[test]
    fn rational_time_validates_negative_rate() {
        let bad = RationalTime::new(1.0, -24.0);
        bad.validate("p.json", &JsonPath::root())
            .expect_err("negative rate should fail");
    }

    #[test]
    fn rational_time_rejects_nan() {
        let bad = RationalTime::new(f64::NAN, 24.0);
        bad.validate("p.json", &JsonPath::root())
            .expect_err("NaN value should fail");
    }

    #[test]
    fn time_range_rejects_negative_duration() {
        let r = TimeRange::new(RationalTime::new(0.0, 24.0), RationalTime::new(-1.0, 24.0));
        let err = r
            .validate("p.json", &JsonPath::root())
            .expect_err("negative duration should fail");
        assert!(err.to_string().contains("duration must be non-negative"));
    }

    #[test]
    fn time_range_zero_duration_ok() {
        let r = TimeRange::new(RationalTime::new(0.0, 24.0), RationalTime::new(0.0, 24.0));
        r.validate("p.json", &JsonPath::root()).unwrap();
    }

    #[test]
    fn rational_time_roundtrip() {
        let t = RationalTime::new(123.0, 24.0);
        let json = serde_json::to_string(&t).unwrap();
        let back: RationalTime = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }
}
