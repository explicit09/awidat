//! Helpers for preview proxy scheduling.

/// Re-order a (id, timeline_start_s, timeline_end_s) asset list so
/// transcoding starts with whatever the user is most likely to play
/// next. Assets the cursor sits on come first (distance 0), then
/// nearest neighbors, then the long-tail off-screen assets. Stable
/// for ties.
pub fn prioritize_for_cursor(assets: &[(String, f64, f64)], cursor_s: f64) -> Vec<String> {
    let mut scored: Vec<(f64, f64, &String)> = assets
        .iter()
        .map(|(id, start, end)| {
            let distance = if cursor_s >= *start && cursor_s <= *end {
                0.0
            } else if cursor_s < *start {
                start - cursor_s
            } else {
                cursor_s - end
            };
            let span = end - start;
            (distance, span, id)
        })
        .collect();
    scored.sort_by(
        |a, b| match a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal) {
            std::cmp::Ordering::Equal => a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal),
            other => other,
        },
    );
    scored.into_iter().map(|(_, _, id)| id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prioritize_orders_assets_by_distance_from_cursor() {
        let assets = vec![
            ("raw/far.mov".to_string(), 0.0, 5.0),
            ("raw/near.mov".to_string(), 9.0, 12.0),
            ("raw/at.mov".to_string(), 10.0, 11.0),
            ("raw/off.mov".to_string(), 30.0, 45.0),
        ];
        let order = prioritize_for_cursor(&assets, 10.5);
        assert_eq!(
            order,
            vec![
                "raw/at.mov".to_string(),
                "raw/near.mov".to_string(),
                "raw/far.mov".to_string(),
                "raw/off.mov".to_string(),
            ],
        );
    }
}
