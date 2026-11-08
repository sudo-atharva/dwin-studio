//! Canva-style smart guides: snap moving edges to other widgets' edges/centres.

/// Smallest shift (|shift| <= `tol`) that puts one of `moving` on one of `targets`.
/// Returns the shift and every target hit by it (to draw the guide lines).
pub fn snap_axis(moving: &[i32], targets: &[i32], tol: i32) -> Option<(i32, Vec<i32>)> {
    let shift = moving
        .iter()
        .flat_map(|m| targets.iter().map(move |t| t - m))
        .filter(|d| d.abs() <= tol)
        .min_by_key(|d| d.abs())?;
    let hits = targets.iter().copied().filter(|t| moving.iter().any(|m| m + shift == *t)).collect();
    Some((shift, hits))
}

/// Left, centre, right (or top, middle, bottom) of a span.
pub fn lines(start: i32, len: i32) -> [i32; 3] {
    [start, start + len / 2, start + len]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_nearest_edge_or_centre() {
        // Box 100..200 moved near another box 300..400: right edge 200 is 3 px from... nothing;
        // its centre 150 is 2 px from target centre 152.
        assert_eq!(snap_axis(&lines(100, 100), &[0, 152, 400], 4), Some((2, vec![152])));
        assert_eq!(snap_axis(&lines(100, 100), &[0, 400], 4), None);
        // Two edges landing at once both get guides.
        assert_eq!(snap_axis(&[10, 50], &[11, 51], 3), Some((1, vec![11, 51])));
    }
}
