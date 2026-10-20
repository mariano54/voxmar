//! A walled place's wall (a size's `walls`, `rules.walls` in settlements.ron): a ring
//! `margin` blocks out round its buildings, a palisade of logs where it is poor and stone
//! where it is rich, towers at its corners and along it, and gates where its lanes pass.

use voxmar_engine::glam::IVec2;

use super::plan::Rect;

/// The corners of the smallest convex ring round `rects`, each grown by `margin`, in turn
/// round it (counter-clockwise, x east and y south); none where there are no rects.
pub(super) fn ring(rects: &[Rect], margin: i32) -> Vec<IVec2> {
    let mut points: Vec<IVec2> = rects
        .iter()
        .flat_map(|r| {
            let r = r.grow(margin);
            [r.min, r.max, IVec2::new(r.min.x, r.max.y), IVec2::new(r.max.x, r.min.y)]
        })
        .collect();
    points.sort_by_key(|p| (p.x, p.y));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |o: IVec2, a: IVec2, b: IVec2| (a.x - o.x) as i64 * (b.y - o.y) as i64 - (a.y - o.y) as i64 * (b.x - o.x) as i64;
    // Andrew's monotone chain: the lower half, then the upper.
    let mut hull: Vec<IVec2> = Vec::with_capacity(points.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        let order: Box<dyn Iterator<Item = &IVec2>> = if pass == 0 { Box::new(points.iter()) } else { Box::new(points.iter().rev()) };
        for &p in order {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

/// The blocks of the ring through `corners`, each touching the next by a side (never only
/// by a corner, so nothing slips through between two), once each, in order round it.
pub(super) fn trace(corners: &[IVec2]) -> Vec<IVec2> {
    let mut cells: Vec<IVec2> = Vec::new();
    for (i, &a) in corners.iter().enumerate() {
        let b = corners[(i + 1) % corners.len()];
        let d = b - a;
        let (n, m) = (d.x.abs(), d.y.abs());
        let step = d.signum();
        // Walks from a to b one block at a time along x or along y, whichever keeps nearer
        // the straight line.
        let (mut ix, mut iy) = (0, 0);
        let mut p = a;
        cells.push(p);
        while ix < n || iy < m {
            // Compare (ix + 0.5) / n against (iy + 0.5) / m without dividing.
            if (iy >= m) || (ix < n && (2 * ix + 1) as i64 * m as i64 <= (2 * iy + 1) as i64 * n as i64) {
                p.x += step.x;
                ix += 1;
            } else {
                p.y += step.y;
                iy += 1;
            }
            if p != b {
                cells.push(p);
            }
        }
    }
    let mut seen = rustc_hash::FxHashSet::default();
    cells.retain(|p| seen.insert(*p));
    cells
}

/// Where along `cells` (by index) towers stand: at each of `corners` (so long as it is no
/// nearer than half of `every` to the last), and every `every` blocks between.
pub(super) fn towers(cells: &[IVec2], corners: &[IVec2], every: usize) -> Vec<usize> {
    let mut at = Vec::new();
    let mut since = every;
    for (i, p) in cells.iter().enumerate() {
        let corner = corners.contains(p) && since * 2 >= every;
        if corner || since >= every {
            at.push(i);
            since = 0;
        }
        since += 1;
    }
    // The last tower and the first may stand too near where the ring closes.
    if at.len() > 1 && (cells.len() - at[at.len() - 1]) * 2 < every {
        at.pop();
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_holds_every_building_and_closes_side_by_side() {
        let rects = [Rect { min: IVec2::new(0, 0), max: IVec2::new(5, 6) }, Rect { min: IVec2::new(20, 3), max: IVec2::new(26, 9) }, Rect { min: IVec2::new(8, 30), max: IVec2::new(12, 34) }];
        let corners = ring(&rects, 3);
        assert!(corners.len() >= 3);
        let cells = trace(&corners);
        for w in cells.windows(2).chain(std::iter::once(&[cells[cells.len() - 1], cells[0]][..])) {
            let d = (w[1] - w[0]).abs();
            assert_eq!(d.x + d.y, 1, "{} and {} do not share a side", w[0], w[1]);
        }
        // Every building and its margin stands inside: a ray from it east crosses the ring.
        let inside = |p: IVec2| {
            let mut crossings = 0;
            for (i, &a) in corners.iter().enumerate() {
                let b = corners[(i + 1) % corners.len()];
                if (a.y > p.y) != (b.y > p.y) {
                    let x = a.x as f32 + (p.y - a.y) as f32 * (b.x - a.x) as f32 / (b.y - a.y) as f32;
                    if (p.x as f32) < x {
                        crossings += 1;
                    }
                }
            }
            crossings % 2 == 1
        };
        for r in &rects {
            for p in [r.min, r.max, IVec2::new(r.min.x, r.max.y), IVec2::new(r.max.x, r.min.y)] {
                assert!(inside(p), "{p} is outside the ring {corners:?}");
                assert!(!cells.iter().any(|c| (*c - p).abs().max_element() < 3), "the ring runs within 3 of {p}");
            }
        }
        let at = towers(&cells, &corners, 10);
        assert!(corners.iter().all(|c| at.iter().any(|&i| (cells[i] - *c).abs().max_element() <= 5)), "a corner with no tower near");
        assert!(at.windows(2).all(|w| w[1] - w[0] <= 10 && w[1] - w[0] >= 5), "{at:?}");
    }
}
