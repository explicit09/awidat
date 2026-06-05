//! Pure color-analysis math for `plan_color_grade`.
//!
//! This module ports the ground-truth correction derivation from
//! `python/packages/color-analysis-mcp` into Rust, with NO I/O and NO
//! async. It consumes a [`ColorScopeSnapshot`] (already computed by the
//! `color_scopes` ffmpeg machinery) and produces frame statistics plus a
//! single bounded [`CorrectionRecommendation`].
//!
//! The recommendation encodes the universal color-craft law: correct
//! first (neutralize exposure, contrast, and casts), and defer any
//! creative saturation push to the look stage (`saturation` stays at the
//! neutral multiplier `1.0`).

use crate::awidat_mcp::tools::color_scopes::ColorScopeSnapshot;

/// Per-frame luminance/channel statistics derived from a scope snapshot.
///
/// All values live on the 0–255 scale (matching the 8-bit RGB the scope
/// machinery samples). Fractions are in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStats {
    /// Count-weighted mean luma.
    pub brightness: f64,
    /// Luma standard deviation (spread).
    pub contrast: f64,
    /// 5th-percentile luma.
    pub luma_p05: f64,
    /// Median luma.
    pub luma_p50: f64,
    /// 95th-percentile luma.
    pub luma_p95: f64,
    /// Mean red channel value.
    pub mean_r: f64,
    /// Mean green channel value.
    pub mean_g: f64,
    /// Mean blue channel value.
    pub mean_b: f64,
    /// Fraction of luma samples at/above 245 (clipped highlights).
    pub overexposed_fraction: f64,
    /// Fraction of luma samples at/below 10 (crushed shadows).
    pub underexposed_fraction: f64,
}

impl FrameStats {
    /// Neutral passthrough stats: mid-gray frame, no clipping. Used as
    /// the aggregate of an empty frame set so the planner stays loud
    /// rather than dividing by zero.
    pub fn neutral() -> Self {
        Self {
            brightness: 128.0,
            contrast: 50.0,
            luma_p05: 16.0,
            luma_p50: 128.0,
            luma_p95: 240.0,
            mean_r: 128.0,
            mean_g: 128.0,
            mean_b: 128.0,
            overexposed_fraction: 0.0,
            underexposed_fraction: 0.0,
        }
    }
}

/// A single bounded color-correction op in canonical registry units.
///
/// Every field is already clamped to the `awidat.color_correction`
/// registry bounds, so callers can emit it without re-validating ranges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CorrectionRecommendation {
    /// Exposure offset in stops, in `[-4.0, 4.0]`.
    pub exposure_ev: f64,
    /// Contrast multiplier (1.0 = neutral), in `[0.0, 3.0]`.
    pub contrast: f64,
    /// Saturation multiplier (1.0 = neutral), in `[0.0, 3.0]`.
    pub saturation: f64,
    /// Warm/cool control, in `[-1.0, 1.0]`.
    pub temperature: f64,
    /// Green/magenta control, in `[-1.0, 1.0]`.
    pub tint: f64,
    /// Shadow lift control, in `[-1.0, 1.0]`.
    pub shadows: f64,
    /// Highlight recovery control, in `[-1.0, 1.0]`.
    pub highlights: f64,
}

/// Derive a bounded correction from frame statistics.
///
/// Ports `_recommend` from the color-analysis MCP verbatim, then clamps
/// every field to the registry bounds. Pure: same input → same output.
pub fn recommended_correction(stats: &FrameStats) -> CorrectionRecommendation {
    let mid_luma = stats.brightness * 0.35 + stats.luma_p50 * 0.65;
    let mut exposure_ev = ((124.0 - mid_luma) / 104.0).clamp(-1.35, 1.35);
    if stats.overexposed_fraction > 0.08 {
        exposure_ev = exposure_ev.min(-0.15);
    }
    if stats.underexposed_fraction > 0.12 {
        exposure_ev = exposure_ev.max(0.25);
    }

    let contrast_span = (stats.luma_p95 - stats.luma_p05).max(1.0);
    let contrast_basis = stats.contrast * 0.45 + contrast_span * 0.55;
    // Neutral point = the basis of a healthy Rec.709 image
    // (stddev ~55 + p05..p95 span ~175 → ~122). Footage flatter than
    // that gets a contrast BOOST; only genuinely harsh footage is
    // reduced, and never below 0.90 — reducing contrast greys/washes the
    // image, which is an anti-pattern for a correction pass, not a look.
    let contrast = (122.0 / contrast_basis.max(1.0)).clamp(0.90, 1.35);

    let rb_delta = stats.mean_r - stats.mean_b;
    let gm_delta = stats.mean_g - ((stats.mean_r + stats.mean_b) / 2.0);
    // Correction opposes the measured excess. The render `colorbalance`
    // makes positive `temperature` warmer (adds red) and positive `tint`
    // magenta (`gs = -tint*0.09` pulls green down). So a warm excess
    // (rb_delta > 0) needs negative temperature, but a GREEN excess
    // (gm_delta > 0) needs POSITIVE tint to push toward magenta. The
    // upstream Python `_recommend` uses `-gm_delta` symmetrically with
    // temperature, which is inverted against this renderer (tracked
    // separately); we use the renderer-correct sign here.
    let temperature = (-rb_delta / 90.0).clamp(-0.85, 0.85);
    let tint = (gm_delta / 80.0).clamp(-0.85, 0.85);
    let shadows = (stats.underexposed_fraction * 2.1).clamp(0.0, 0.65);
    let highlights = (-stats.overexposed_fraction * 2.2).clamp(-0.65, 0.0);

    // Correction defers saturation to the look stage.
    let saturation: f64 = 1.0;

    // Final clamp to the registry bounds (a belt-and-braces guard; the
    // per-field clamps above already keep us well inside these ranges).
    CorrectionRecommendation {
        exposure_ev: exposure_ev.clamp(-4.0, 4.0),
        contrast: contrast.clamp(0.0, 3.0),
        saturation: saturation.clamp(0.0, 3.0),
        temperature: temperature.clamp(-1.0, 1.0),
        tint: tint.clamp(-1.0, 1.0),
        shadows: shadows.clamp(-1.0, 1.0),
        highlights: highlights.clamp(-1.0, 1.0),
    }
}

/// Bin index → representative 0–255 value (bin center).
fn bin_center(bin: usize, bins: usize) -> f64 {
    // Each bin spans 256/bins units of the 0–255 axis. The center of bin
    // `i` is `(i + 0.5) * 256 / bins`, clamped to the valid range.
    (((bin as f64) + 0.5) * 256.0 / bins as f64).clamp(0.0, 255.0)
}

/// Count-weighted mean of a histogram over 0–255 bin centers.
fn histogram_mean(counts: &[u32]) -> f64 {
    let bins = counts.len();
    if bins == 0 {
        return 128.0;
    }
    let mut total = 0u64;
    let mut weighted = 0.0f64;
    for (bin, &count) in counts.iter().enumerate() {
        total += u64::from(count);
        weighted += bin_center(bin, bins) * f64::from(count);
    }
    if total == 0 {
        return 128.0;
    }
    weighted / total as f64
}

/// Count-weighted standard deviation of a histogram.
fn histogram_stddev(counts: &[u32], mean: f64) -> f64 {
    let bins = counts.len();
    if bins == 0 {
        return 0.0;
    }
    let mut total = 0u64;
    let mut acc = 0.0f64;
    for (bin, &count) in counts.iter().enumerate() {
        let d = bin_center(bin, bins) - mean;
        acc += d * d * f64::from(count);
        total += u64::from(count);
    }
    if total == 0 {
        return 0.0;
    }
    (acc / total as f64).sqrt()
}

/// Percentile (0–100) value from a histogram via the cumulative count.
fn histogram_percentile(counts: &[u32], pct: f64) -> f64 {
    let bins = counts.len();
    if bins == 0 {
        return 128.0;
    }
    let total: u64 = counts.iter().map(|&c| u64::from(c)).sum();
    if total == 0 {
        return 128.0;
    }
    let target = (pct / 100.0) * total as f64;
    let mut cumulative = 0.0f64;
    for (bin, &count) in counts.iter().enumerate() {
        cumulative += f64::from(count);
        if cumulative >= target {
            return bin_center(bin, bins);
        }
    }
    bin_center(bins - 1, bins)
}

/// Fraction of luma samples in bins whose center value is `>= threshold`.
fn fraction_at_or_above(counts: &[u32], threshold: f64) -> f64 {
    let bins = counts.len();
    let total: u64 = counts.iter().map(|&c| u64::from(c)).sum();
    if total == 0 {
        return 0.0;
    }
    let mut hit = 0u64;
    for (bin, &count) in counts.iter().enumerate() {
        if bin_center(bin, bins) >= threshold {
            hit += u64::from(count);
        }
    }
    hit as f64 / total as f64
}

/// Fraction of luma samples in bins whose center value is `<= threshold`.
fn fraction_at_or_below(counts: &[u32], threshold: f64) -> f64 {
    let bins = counts.len();
    let total: u64 = counts.iter().map(|&c| u64::from(c)).sum();
    if total == 0 {
        return 0.0;
    }
    let mut hit = 0u64;
    for (bin, &count) in counts.iter().enumerate() {
        if bin_center(bin, bins) <= threshold {
            hit += u64::from(count);
        }
    }
    hit as f64 / total as f64
}

/// Derive [`FrameStats`] from a single scope snapshot. Pure histogram math.
pub fn stats_from_snapshot(snapshot: &ColorScopeSnapshot) -> FrameStats {
    let luma = &snapshot.luma_histogram;
    let brightness = histogram_mean(luma);
    let contrast = histogram_stddev(luma, brightness);
    FrameStats {
        brightness,
        contrast,
        luma_p05: histogram_percentile(luma, 5.0),
        luma_p50: histogram_percentile(luma, 50.0),
        luma_p95: histogram_percentile(luma, 95.0),
        mean_r: histogram_mean(&snapshot.rgb_histogram.red),
        mean_g: histogram_mean(&snapshot.rgb_histogram.green),
        mean_b: histogram_mean(&snapshot.rgb_histogram.blue),
        overexposed_fraction: fraction_at_or_above(luma, 245.0),
        underexposed_fraction: fraction_at_or_below(luma, 10.0),
    }
}

/// Mean of a set of frame stats. Empty input yields neutral passthrough.
pub fn aggregate_stats(frames: &[FrameStats]) -> FrameStats {
    if frames.is_empty() {
        return FrameStats::neutral();
    }
    let n = frames.len() as f64;
    let mut acc = FrameStats {
        brightness: 0.0,
        contrast: 0.0,
        luma_p05: 0.0,
        luma_p50: 0.0,
        luma_p95: 0.0,
        mean_r: 0.0,
        mean_g: 0.0,
        mean_b: 0.0,
        overexposed_fraction: 0.0,
        underexposed_fraction: 0.0,
    };
    for f in frames {
        acc.brightness += f.brightness;
        acc.contrast += f.contrast;
        acc.luma_p05 += f.luma_p05;
        acc.luma_p50 += f.luma_p50;
        acc.luma_p95 += f.luma_p95;
        acc.mean_r += f.mean_r;
        acc.mean_g += f.mean_g;
        acc.mean_b += f.mean_b;
        acc.overexposed_fraction += f.overexposed_fraction;
        acc.underexposed_fraction += f.underexposed_fraction;
    }
    FrameStats {
        brightness: acc.brightness / n,
        contrast: acc.contrast / n,
        luma_p05: acc.luma_p05 / n,
        luma_p50: acc.luma_p50 / n,
        luma_p95: acc.luma_p95 / n,
        mean_r: acc.mean_r / n,
        mean_g: acc.mean_g / n,
        mean_b: acc.mean_b / n,
        overexposed_fraction: acc.overexposed_fraction / n,
        underexposed_fraction: acc.underexposed_fraction / n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awidat_mcp::tools::color_scopes::{ColorScopeInput, compute_color_scopes};

    /// Neutral, well-exposed mid-gray frame stats.
    fn neutral_stats() -> FrameStats {
        FrameStats {
            brightness: 124.0,
            contrast: 60.0,
            luma_p05: 100.0,
            luma_p50: 124.0,
            luma_p95: 150.0,
            mean_r: 128.0,
            mean_g: 128.0,
            mean_b: 128.0,
            overexposed_fraction: 0.0,
            underexposed_fraction: 0.0,
        }
    }

    fn within(value: f64, lo: f64, hi: f64) -> bool {
        value >= lo - 1e-9 && value <= hi + 1e-9
    }

    fn assert_bounds(rec: &CorrectionRecommendation) {
        assert!(within(rec.exposure_ev, -4.0, 4.0), "exposure_ev oob");
        assert!(within(rec.contrast, 0.0, 3.0), "contrast oob");
        assert!(within(rec.saturation, 0.0, 3.0), "saturation oob");
        assert!(within(rec.temperature, -1.0, 1.0), "temperature oob");
        assert!(within(rec.tint, -1.0, 1.0), "tint oob");
        assert!(within(rec.shadows, -1.0, 1.0), "shadows oob");
        assert!(within(rec.highlights, -1.0, 1.0), "highlights oob");
    }

    #[test]
    fn neutral_frame_recommends_near_zero_corrections() {
        let rec = recommended_correction(&neutral_stats());
        assert!(
            rec.exposure_ev.abs() < 0.1,
            "exposure_ev={}",
            rec.exposure_ev
        );
        assert!(
            rec.temperature.abs() < 0.05,
            "temperature={}",
            rec.temperature
        );
        assert!(rec.tint.abs() < 0.05, "tint={}", rec.tint);
        assert_eq!(rec.shadows, 0.0);
        assert_eq!(rec.highlights, 0.0);
        assert_eq!(rec.saturation, 1.0, "saturation deferred to look");
        assert_bounds(&rec);
    }

    #[test]
    fn underexposed_lifts_exposure_and_shadows() {
        let mut stats = neutral_stats();
        stats.brightness = 40.0;
        stats.luma_p50 = 38.0;
        stats.underexposed_fraction = 0.30;
        let rec = recommended_correction(&stats);
        assert!(rec.exposure_ev >= 0.25, "exposure_ev={}", rec.exposure_ev);
        assert!(rec.shadows > 0.0, "shadows={}", rec.shadows);
        assert_bounds(&rec);
    }

    #[test]
    fn overexposed_drops_exposure_and_highlights() {
        let mut stats = neutral_stats();
        stats.brightness = 220.0;
        stats.luma_p50 = 225.0;
        stats.overexposed_fraction = 0.20;
        let rec = recommended_correction(&stats);
        assert!(rec.exposure_ev < -0.15, "exposure_ev={}", rec.exposure_ev);
        assert!(rec.highlights < 0.0, "highlights={}", rec.highlights);
        assert_bounds(&rec);
    }

    #[test]
    fn warm_cast_cools_temperature() {
        let mut stats = neutral_stats();
        stats.mean_r = 170.0;
        stats.mean_b = 90.0;
        let rec = recommended_correction(&stats);
        assert!(rec.temperature < 0.0, "temperature={}", rec.temperature);
        assert_bounds(&rec);
    }

    #[test]
    fn green_cast_pushes_tint_positive_toward_magenta() {
        // A green cast (g above the r/b average) must be corrected by
        // pushing toward magenta, which is POSITIVE tint in the render
        // colorbalance (gs = -tint*0.09 pulls green down).
        let mut stats = neutral_stats();
        stats.mean_g = 170.0;
        stats.mean_r = 120.0;
        stats.mean_b = 120.0;
        let rec = recommended_correction(&stats);
        assert!(rec.tint > 0.0, "tint={}", rec.tint);
        assert_bounds(&rec);
    }

    #[test]
    fn low_contrast_raises_contrast_above_one() {
        let mut stats = neutral_stats();
        stats.contrast = 20.0;
        stats.luma_p05 = 110.0;
        stats.luma_p95 = 140.0;
        let rec = recommended_correction(&stats);
        assert!(rec.contrast > 1.0, "contrast={}", rec.contrast);
        assert_bounds(&rec);
    }

    #[test]
    fn normal_contrast_is_not_crushed_into_grey() {
        // A normally-exposed clip (short6's real measured profile:
        // stddev ~51, p05 35, p95 188 → basis ~107) must NOT have its
        // contrast pulled down — that washes the image toward grey.
        // It should sit at neutral-or-a-touch-of-boost.
        let mut stats = neutral_stats();
        stats.contrast = 51.0;
        stats.luma_p05 = 35.0;
        stats.luma_p50 = 75.0;
        stats.luma_p95 = 188.0;
        let rec = recommended_correction(&stats);
        assert!(
            rec.contrast >= 1.0,
            "normal footage must not lose contrast: contrast={}",
            rec.contrast
        );
        assert!(
            rec.contrast <= 1.25,
            "contrast boost too aggressive: {}",
            rec.contrast
        );
    }

    #[test]
    fn bounds_are_never_exceeded_under_fuzz() {
        // Sweep extreme inputs; every field must stay in registry bounds.
        let samples = [
            (0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0),
            (255.0, 255.0, 0.0, 255.0, 0.0, 1.0, 1.0),
            (0.0, 255.0, 255.0, 0.0, 255.0, 1.0, 1.0),
            (128.0, 0.0, 255.0, 128.0, 128.0, 0.0, 0.0),
            (255.0, 0.0, 128.0, 0.0, 255.0, 0.5, 0.5),
        ];
        for (b, r, g, bl, p50, over, under) in samples {
            let stats = FrameStats {
                brightness: b,
                contrast: 1.0,
                luma_p05: 0.0,
                luma_p50: p50,
                luma_p95: 255.0,
                mean_r: r,
                mean_g: g,
                mean_b: bl,
                overexposed_fraction: over,
                underexposed_fraction: under,
            };
            let rec = recommended_correction(&stats);
            assert_bounds(&rec);
        }
    }

    #[test]
    fn recommendation_is_pure() {
        let stats = neutral_stats();
        let a = recommended_correction(&stats);
        let b = recommended_correction(&stats);
        assert_eq!(a, b);
    }

    // --- stats_from_snapshot via real scope computation ---

    fn snapshot_from_solid(r: u8, g: u8, b: u8) -> ColorScopeSnapshot {
        let width = 16usize;
        let height = 16usize;
        let mut rgb = Vec::with_capacity(width * height * 3);
        for _ in 0..(width * height) {
            rgb.push(r);
            rgb.push(g);
            rgb.push(b);
        }
        let input = ColorScopeInput {
            width,
            height,
            bins: 64,
            rgb,
        };
        compute_color_scopes(&input).unwrap()
    }

    #[test]
    fn gray_frame_stats_are_mid_with_no_clipping() {
        let snap = snapshot_from_solid(128, 128, 128);
        let stats = stats_from_snapshot(&snap);
        assert!(
            (stats.brightness - 128.0).abs() < 4.0,
            "{}",
            stats.brightness
        );
        assert!(stats.overexposed_fraction < 1e-9);
        assert!(stats.underexposed_fraction < 1e-9);
        assert!((stats.luma_p50 - 128.0).abs() < 4.0, "{}", stats.luma_p50);
    }

    #[test]
    fn white_frame_is_fully_overexposed() {
        let snap = snapshot_from_solid(255, 255, 255);
        let stats = stats_from_snapshot(&snap);
        assert!((stats.overexposed_fraction - 1.0).abs() < 1e-9);
    }

    #[test]
    fn black_frame_is_fully_underexposed() {
        let snap = snapshot_from_solid(0, 0, 0);
        let stats = stats_from_snapshot(&snap);
        assert!((stats.underexposed_fraction - 1.0).abs() < 1e-9);
    }

    #[test]
    fn warm_frame_has_red_above_blue() {
        let snap = snapshot_from_solid(200, 130, 60);
        let stats = stats_from_snapshot(&snap);
        assert!(
            stats.mean_r > stats.mean_b,
            "r={} b={}",
            stats.mean_r,
            stats.mean_b
        );
    }

    // --- aggregate_stats ---

    #[test]
    fn aggregate_of_two_frames_is_the_midpoint() {
        let a = FrameStats {
            brightness: 100.0,
            contrast: 40.0,
            luma_p05: 20.0,
            luma_p50: 100.0,
            luma_p95: 180.0,
            mean_r: 100.0,
            mean_g: 110.0,
            mean_b: 120.0,
            overexposed_fraction: 0.0,
            underexposed_fraction: 0.2,
        };
        let mut b = a;
        b.brightness = 200.0;
        b.mean_r = 200.0;
        b.underexposed_fraction = 0.0;
        let agg = aggregate_stats(&[a, b]);
        assert!((agg.brightness - 150.0).abs() < 1e-9);
        assert!((agg.mean_r - 150.0).abs() < 1e-9);
        assert!((agg.underexposed_fraction - 0.1).abs() < 1e-9);
    }

    #[test]
    fn aggregate_of_empty_is_neutral() {
        let agg = aggregate_stats(&[]);
        assert_eq!(agg, FrameStats::neutral());
    }
}
