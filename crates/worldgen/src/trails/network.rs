//! Which places trails join, and how trails that run close together merge.
//!
//! **Edges.** Two places are joined when they are near enough and no third place lies in
//! the circle on the line between them (the Gabriel graph, which keeps the network
//! sparse but connected), and when its lower end has no shorter trail leaving within 35°
//! of it, so two trails do not set off side by side (at the upper end, snapping merges
//! them instead). Every test reads only the places around the two ends, so whichever
//! region asks, the answer is the same. A trail belongs to the region of its lower end,
//! which routes it.
//!
//! **Snapping** (RoadWeaver's merge, MIT): where a trail comes within 18 blocks of a
//! longer one, it joins it and follows it until they part by more than 30, so trails to
//! the same summit share one tread instead of wearing parallel ruts. The shorter trail
//! is cut where it joins, making a junction.

use voxmar_engine::glam::{IVec2, Vec2};

use super::sites::{CELL, Poi};

/// Longest trail between two places, in blocks, as the crow flies.
pub const MAX_EDGE: i32 = 360;
/// Smallest angle between two trails leaving one place.
const MIN_ANGLE_COS: f32 = 0.819; // cos 35°
/// A trail joins a longer one this close, and leaves it this far apart. Routes are on an
/// 8-block grid, so trails a cell or two apart, which would be drawn side by side, merge.
const SNAP: f32 = 18.0;
const UNSNAP: f32 = 30.0;

/// Whether the trail from `a` to `b` belongs in the network. `near(centre, radius)`
/// lists the places within `radius` blocks of `centre` (it may list more).
pub fn keep(a: &Poi, b: &Poi, near: &dyn Fn(Vec2, f32) -> Vec<Poi>) -> bool {
    let (pa, pb) = (a.at().as_vec2(), b.at().as_vec2());
    let length = pa.distance(pb);
    if length > MAX_EDGE as f32 || length < CELL as f32 {
        return false;
    }
    gabriel(a, b, near) && !crowded(a, b, near)
}

/// No place lies in the circle whose diameter is the line from `a` to `b`.
fn gabriel(a: &Poi, b: &Poi, near: &dyn Fn(Vec2, f32) -> Vec<Poi>) -> bool {
    let (pa, pb) = (a.at().as_vec2(), b.at().as_vec2());
    let ab = pa.distance_squared(pb);
    near((pa + pb) / 2.0, pa.distance(pb) / 2.0 + 1.0).iter().all(|c| {
        let pc = c.at().as_vec2();
        c == a || c == b || pa.distance_squared(pc) + pb.distance_squared(pc) > ab
    })
}

/// Whether a shorter edge of the graph leaves `a` within the minimum angle of `b`.
fn crowded(a: &Poi, b: &Poi, near: &dyn Fn(Vec2, f32) -> Vec<Poi>) -> bool {
    let (pa, pb) = (a.at().as_vec2(), b.at().as_vec2());
    let (dir, length) = ((pb - pa).normalize(), pa.distance(pb));
    near(pa, length).iter().any(|c| {
        let pc = c.at().as_vec2();
        let d = pa.distance(pc);
        let shorter = d < length || (d == length && (c.cell.x, c.cell.y) < (b.cell.x, b.cell.y));
        c != a && c != b && d >= CELL as f32 && shorter && (pc - pa).normalize().dot(dir) > MIN_ANGLE_COS && gabriel(a, c, near)
    })
}

/// Where a point lies nearest on a polyline: the point and its distance.
fn nearest_on(p: Vec2, line: &[Vec2]) -> (Vec2, f32) {
    let mut best = (line[0], p.distance(line[0]));
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
        let q = a + ab * t;
        let d = p.distance(q);
        if d < best.1 {
            best = (q, d);
        }
    }
    best
}

/// The pieces of `trail` (points in blocks) left after it joins the longer trails
/// `trunks`, each piece ending where it meets a trunk.
pub fn snap(trail: &[Vec2], trunks: &[&[Vec2]]) -> Vec<Vec<Vec2>> {
    let nearest: Vec<(Vec2, f32)> = trail
        .iter()
        .map(|&p| trunks.iter().map(|t| nearest_on(p, t)).min_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or((p, f32::MAX)))
        .collect();
    let mut on = vec![false; trail.len()];
    let mut snapped = false;
    for (i, &(_, d)) in nearest.iter().enumerate() {
        snapped = if snapped { d <= UNSNAP } else { d <= SNAP };
        on[i] = snapped;
    }
    // A brief brush with another trail is not a junction.
    let mut i = 0;
    while i < on.len() {
        let run = on[i..].iter().take_while(|&&s| s == on[i]).count();
        if on[i] && run < 2 {
            on[i..i + run].iter_mut().for_each(|s| *s = false);
        }
        i += run;
    }
    let mut pieces = Vec::new();
    let mut piece: Vec<Vec2> = Vec::new();
    for i in 0..trail.len() {
        if on[i] {
            if !piece.is_empty() {
                // Run on to where this trail meets the trunk.
                piece.push(nearest[i].0);
                pieces.push(std::mem::take(&mut piece));
            }
            continue;
        }
        if piece.is_empty() && i > 0 && on[i - 1] {
            piece.push(nearest[i - 1].0);
        }
        piece.push(trail[i]);
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    pieces.retain(|p| p.len() >= 2);
    pieces
}

/// Grid cells to points in blocks.
pub fn points(cells: &[IVec2]) -> Vec<Vec2> {
    cells.iter().map(|c| (*c * CELL).as_vec2()).collect()
}

#[cfg(test)]
mod tests {
    use super::super::sites::Kind;
    use super::*;

    fn poi(x: i32, z: i32) -> Poi {
        Poi { cell: IVec2::new(x, z), height: 140, kind: Kind::Trailhead }
    }

    #[test]
    fn keeps_a_sparse_graph_without_parallel_spokes() {
        let all = [poi(0, 0), poi(20, 0), poi(10, 1), poi(0, 30), poi(25, 3)];
        let near = |_: Vec2, _: f32| all.to_vec();
        // (10, 1) lies in the circle between (0, 0) and (20, 0).
        assert!(!keep(&all[0], &all[1], &near));
        assert!(keep(&all[0], &all[2], &near));
        assert!(keep(&all[0], &all[3], &near));
        // (20, 0) to (25, 3) and (20, 0) to (10, 1) are fine; (0,0) to (25,3) is too long
        // past others and blocked.
        assert!(!keep(&all[0], &all[4], &near));
        // Too far apart.
        assert!(!keep(&poi(0, 0), &poi(100, 0), &|_, _| vec![]));
    }

    #[test]
    fn a_trail_joins_a_longer_one_and_stops_there() {
        let trunk: Vec<Vec2> = (0..=20).map(|i| Vec2::new(i as f32 * 8.0, 0.0)).collect();
        // Comes in from the north, then runs beside the trunk 6 blocks away to its end.
        let mut trail: Vec<Vec2> = (0..6).map(|i| Vec2::new(40.0, 80.0 - i as f32 * 14.0)).collect();
        trail.extend((0..10).map(|i| Vec2::new(48.0 + i as f32 * 8.0, 6.0)));
        let pieces = snap(&trail, &[&trunk]);
        assert_eq!(pieces.len(), 1, "{pieces:?}");
        let piece = &pieces[0];
        assert_eq!(piece[0], trail[0]);
        // It ends on the trunk.
        assert!(piece.last().unwrap().y.abs() < 1e-3, "{piece:?}");
        assert!(piece.len() < trail.len());
        // Far from any trunk, nothing changes.
        assert_eq!(snap(&trail, &[]), vec![trail.clone()]);
    }
}
