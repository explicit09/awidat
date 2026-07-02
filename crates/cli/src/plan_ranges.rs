//! Shared source-range planning helpers for deterministic EDL planners.

use crate::plan_clip::SelectedClip;

/// Source-media range in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceRange {
    /// Inclusive source start, in media seconds.
    pub start_s: f64,
    /// Exclusive source end, in media seconds.
    pub end_s: f64,
}

/// Build kept source ranges after removing the requested source spans.
pub fn kept_ranges_after_removing(
    clip: &SelectedClip,
    removed_ranges: Vec<SourceRange>,
) -> Vec<SourceRange> {
    let cuts = merge_ranges(removed_ranges);
    if cuts.is_empty() {
        return vec![SourceRange {
            start_s: clip.source_start_s,
            end_s: clip.source_end_s,
        }];
    }

    let mut kept = Vec::new();
    let mut cursor_s = clip.source_start_s;
    for cut in cuts {
        push_non_empty_range(&mut kept, cursor_s, cut.start_s);
        cursor_s = cursor_s.max(cut.end_s);
    }
    push_non_empty_range(&mut kept, cursor_s, clip.source_end_s);
    kept
}

/// Add a non-empty source range.
pub fn push_non_empty_range(ranges: &mut Vec<SourceRange>, start_s: f64, end_s: f64) {
    const MIN_RANGE_S: f64 = 0.010;
    if end_s - start_s >= MIN_RANGE_S {
        ranges.push(SourceRange { start_s, end_s });
    }
}

/// Merge overlapping source ranges.
pub fn merge_ranges(mut ranges: Vec<SourceRange>) -> Vec<SourceRange> {
    ranges.sort_by(|left, right| left.start_s.total_cmp(&right.start_s));
    let mut merged: Vec<SourceRange> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start_s <= last.end_s
        {
            last.end_s = last.end_s.max(range.end_s);
            continue;
        }
        merged.push(range);
    }
    merged
}

/// Render kept source ranges as Trim + Insert Clip EDL operations.
pub fn build_kept_ranges_edl(
    clip: &SelectedClip,
    kept_ranges: &[SourceRange],
    inserted_name_prefix: &str,
) -> String {
    let mut edl = String::from("*** Begin EDL\n");
    append_kept_ranges_edl_ops(&mut edl, clip, kept_ranges, inserted_name_prefix);
    edl.push_str("*** End EDL\n");
    edl
}

/// Append Trim + Insert Clip EDL operations for changed kept ranges.
pub fn append_kept_ranges_edl_ops(
    edl: &mut String,
    clip: &SelectedClip,
    kept_ranges: &[SourceRange],
    inserted_name_prefix: &str,
) -> usize {
    append_kept_ranges_edl_ops_with_position_offset(edl, clip, kept_ranges, inserted_name_prefix, 0)
}

/// Append Trim + Insert Clip EDL operations, offsetting absolute insert positions.
pub fn append_kept_ranges_edl_ops_with_position_offset(
    edl: &mut String,
    clip: &SelectedClip,
    kept_ranges: &[SourceRange],
    inserted_name_prefix: &str,
    position_offset: usize,
) -> usize {
    if kept_ranges_match_original(clip, kept_ranges) {
        return 0;
    }
    let mut inserted_count = 0;
    if let Some(first) = kept_ranges.first() {
        edl.push_str("*** Trim Clip\n");
        edl.push_str(&format!("@@ anchor: clip_uuid={}\n", clip.clip_uuid));
        edl.push_str(&format!("+ start: {:.3}\n", first.start_s));
        edl.push_str(&format!("+ end: {:.3}\n", first.end_s));
        for (index, range) in kept_ranges.iter().enumerate().skip(1) {
            let at_position = clip.track_position + position_offset + index;
            edl.push_str("*** Insert Clip\n");
            edl.push_str(&format!("+ asset: {}\n", clip.asset));
            edl.push_str(&format!("+ track: {}\n", clip.track_name));
            edl.push_str(&format!("+ at_position: {at_position}\n"));
            edl.push_str(&format!("+ start: {:.3}\n", range.start_s));
            edl.push_str(&format!("+ end: {:.3}\n", range.end_s));
            edl.push_str(&format!(
                "+ name: {}-{inserted_name_prefix}-{index}\n",
                clip.clip_uuid
            ));
            inserted_count += 1;
        }
    }
    inserted_count
}

fn kept_ranges_match_original(clip: &SelectedClip, kept_ranges: &[SourceRange]) -> bool {
    const EPSILON_S: f64 = 0.001;
    let [only] = kept_ranges else {
        return false;
    };
    (only.start_s - clip.source_start_s).abs() <= EPSILON_S
        && (only.end_s - clip.source_end_s).abs() <= EPSILON_S
}
