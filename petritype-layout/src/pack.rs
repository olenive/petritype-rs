//! The packing of open groups in rows.

use crate::Point;

/// The shape of window the packing of groups aims for: width over height.
const LANDSCAPE: f32 = 1.6;

/// The rule a [`Layout`](crate::Layout) packs its open groups with, after
/// the nodes in no group. It is public so that a picture of the same net in
/// other units, such as a thumbnail's grid of cells, can pack the groups
/// alike.
///
/// Packs boxes of the given sizes in rows, in the order given, with `gap`
/// between neighbours both across and down, and returns the top-left corner
/// of each box, in the same order, in world coordinates (y up). The first
/// row starts at the origin and the rows go down, so no corner has a `y`
/// above zero.
///
/// The row width is the one that makes the whole frame largest on a screen
/// 1.6 times as wide as high, that is, the one minimising the larger of its
/// width over 1.6 and its height, then its area. The candidates are the
/// widths of every run of consecutive boxes, and the first of equal
/// candidates wins, so the result depends on the sizes and the gap alone.
///
/// ```
/// use petritype_layout::{Point, pack};
///
/// // Four unit squares one apart pack two by two.
/// let corners = pack(&[Point::new(1.0, 1.0); 4], 1.0);
/// let want = [(0.0, 0.0), (2.0, 0.0), (0.0, -2.0), (2.0, -2.0)].map(|(x, y)| Point::new(x, y));
/// assert_eq!(corners, want);
/// ```
pub fn pack(sizes: &[Point], gap: f32) -> Vec<Point> {
    let widest = sizes.iter().fold(0.0_f32, |w, s| w.max(s.x));
    let mut best: Option<(f32, f32, Vec<Point>)> = None;
    for start in 0..sizes.len() {
        let mut limit = -gap;
        for size in &sizes[start..] {
            limit += gap + size.x;
            if limit < widest {
                continue;
            }
            let (corners, width, height) = pack_rows(sizes, limit, gap);
            let score = (width / LANDSCAPE).max(height);
            let better = best
                .as_ref()
                .is_none_or(|(s, a, _)| (score, width * height) < (*s, *a));
            if better {
                best = Some((score, width * height, corners));
            }
        }
    }
    best.map_or_else(
        || pack_rows(sizes, widest, gap).0,
        |(_, _, corners)| corners,
    )
}

/// Fills rows no wider than `limit` (a row always takes at least one box),
/// and returns the corners and the width and height of the whole.
fn pack_rows(sizes: &[Point], limit: f32, gap: f32) -> (Vec<Point>, f32, f32) {
    let mut corners = Vec::with_capacity(sizes.len());
    let (mut x, mut y, mut row_height, mut width) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
    for size in sizes {
        if x > 0.0 && x + size.x > limit {
            y += row_height + gap;
            x = 0.0;
            row_height = 0.0;
        }
        corners.push(Point::new(x, -y));
        x += size.x + gap;
        width = width.max(x - gap);
        row_height = row_height.max(size.y);
    }
    (corners, width, y + row_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_is_deterministic_and_tends_to_landscape() {
        // Nine equal boxes pack to three by three or wider, never a column.
        let sizes = vec![Point::new(100.0, 100.0); 9];
        let corners = pack(&sizes, 10.0);
        assert_eq!(corners, pack(&sizes, 10.0));
        let width = corners.iter().map(|c| c.x + 100.0).fold(0.0, f32::max);
        let height = corners.iter().map(|c| 100.0 - c.y).fold(0.0, f32::max);
        assert!(width >= height * 0.9, "{width} x {height}");
        assert!(width <= height * 3.0, "{width} x {height}");
    }
}
