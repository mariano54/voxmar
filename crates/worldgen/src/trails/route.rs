//! Routing a trail between two places: A* over the planning grid, with the cost terms of
//! RoadWeaver's potential-field pathfinder (MIT, see THIRD_PARTY_NOTICES.md), retuned for
//! footpaths:
//!
//! - walking along the contour is cheaper than across it, by up to 45% where the land
//!   is steep;
//! - grades past a soft limit cost more, and past a hard limit far more;
//! - each step's rise costs its square, so one big step costs more than several small;
//! - heading straight up or down the fall line costs extra.
//!
//! Together these make trails traverse a slope and switch back rather than climb it head
//! on, with no switchback code at all. Water is forded where shallow and bridged where
//! deep, at a price, and the sea is never crossed.
//!
//! Costs are whole numbers (fixed point), and ties are broken by position, so a route is
//! the same on every machine. A route that does not arrive within its search box and
//! budget is no route: the trail is not built, the same everywhere.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use voxmar_engine::glam::{IVec2, Vec2};

use super::sites::{CELL, Cell};

/// Fixed-point units per block of flat walking.
const UNIT: f32 = 64.0;
/// Grades (rise over run) past which walking costs more, and far more.
const SOFT_GRADE: f32 = 0.15;
const HARD_GRADE: f32 = 0.35;
/// Steeper than this a trail only scrambles, at a high price (summits are often
/// craggy), and steeper than the cliff grade it cannot go at all.
const SCRAMBLE_GRADE: f32 = 1.0;
const CLIFF_GRADE: f32 = 2.0;
/// What turning back on itself costs, in blocks of flat walking.
const TURN: f32 = 40.0;
/// Deepest water a trail fords; deeper water is bridged.
pub const FORD_DEPTH: i32 = 1;
/// Deepest water a bridge spans.
const BRIDGE_DEPTH: i32 = 12;

/// Moves from a cell: the eight neighbours, and the eight knight's moves, which let a trail
/// climb a slope at a shallower angle than a diagonal can.
const STEPS: [IVec2; 16] = [
    IVec2::new(1, 0),
    IVec2::new(-1, 0),
    IVec2::new(0, 1),
    IVec2::new(0, -1),
    IVec2::new(1, 1),
    IVec2::new(1, -1),
    IVec2::new(-1, 1),
    IVec2::new(-1, -1),
    IVec2::new(2, 1),
    IVec2::new(2, -1),
    IVec2::new(-2, 1),
    IVec2::new(-2, -1),
    IVec2::new(1, 2),
    IVec2::new(1, -2),
    IVec2::new(-1, 2),
    IVec2::new(-1, -2),
];

/// The cost of walking from `from` to the neighbouring cell `to`, in fixed point, or
/// `None` where a trail cannot go. `gradient` is the slope of the land at `from`, rise per
/// block along x and z.
pub fn step_cost(from: &Cell, to: &Cell, step: IVec2, gradient: Vec2) -> Option<u32> {
    if to.sea() && to.depth() > FORD_DEPTH {
        return None;
    }
    let run = step.as_vec2().length() * CELL as f32;
    // On water, the surface is walked (stones or a deck), not the bed.
    let level = |c: &Cell| if c.submerged() { c.water } else { c.height } as f32;
    let rise = (level(to) - level(from)).abs();
    let grade = rise / run;
    if grade > CLIFF_GRADE {
        return None;
    }
    let direction = step.as_vec2().normalize();
    let steepness = gradient.length();
    // Along the contour is cheaper the steeper the land.
    let contour = if steepness > 1e-4 { Vec2::new(-gradient.y, gradient.x) / steepness } else { Vec2::ZERO };
    let along = direction.dot(contour).abs();
    let mut cost = run * (1.0 - along * 0.45 * (steepness * 2.0).min(1.0)).max(0.3);
    if grade > SOFT_GRADE {
        cost += run * 6.0 * (grade - SOFT_GRADE) / SOFT_GRADE;
    }
    if grade > HARD_GRADE {
        cost += run * 40.0 * (grade - HARD_GRADE) / HARD_GRADE;
    }
    if grade > SCRAMBLE_GRADE {
        cost += run * 150.0 * (grade - SCRAMBLE_GRADE);
    }
    cost += rise * rise * 0.08;
    // Straight up or down the fall line.
    if steepness > 1e-4 {
        cost += direction.dot(gradient / steepness).abs() * steepness * run * 3.0;
    }
    if to.submerged() {
        let depth = to.depth();
        if depth > BRIDGE_DEPTH {
            return None;
        }
        cost += if depth <= FORD_DEPTH { run * 2.0 } else { run * 4.0 + (depth * depth) as f32 * 0.5 };
    }
    Some((cost * UNIT).round() as u32)
}

/// The cheapest route from `start` to `goal` (grid coordinates) within `min..=max`, as
/// the cells walked, both ends included. `cell` gives the grid; `budget` caps the cells
/// expanded.
pub fn route(start: IVec2, goal: IVec2, min: IVec2, max: IVec2, budget: usize, cell: &dyn Fn(IVec2) -> Cell) -> Option<Vec<IVec2>> {
    let size = max - min + 1;
    let index = |c: IVec2| {
        let l = c - min;
        (l.x >= 0 && l.y >= 0 && l.x < size.x && l.y < size.y).then(|| (l.y * size.x + l.x) as usize)
    };
    let (start_i, goal_i) = (index(start)?, index(goal)?);
    let n = (size.x * size.y) as usize;
    let mut cost = vec![u32::MAX; n];
    let mut from = vec![u8::MAX; n];
    let mut done = vec![false; n];
    // Cells are read many times; keep them.
    let mut cells: Vec<Option<Cell>> = vec![None; n];
    let mut get = |c: IVec2, i: usize| *cells[i].get_or_insert_with(|| cell(c));
    // Slightly overestimates (flat walking costs 1 per block, contours as little as 0.3),
    // which keeps the search quick at little cost to the route.
    let estimate = |c: IVec2| ((c - goal).as_vec2().length() * CELL as f32 * 0.8 * UNIT) as u32;
    let mut open = BinaryHeap::new();
    cost[start_i] = 0;
    open.push(Reverse((estimate(start), 0u32, start.x, start.y)));
    let mut expanded = 0;
    while let Some(Reverse((_, g, x, z))) = open.pop() {
        let c = IVec2::new(x, z);
        let i = index(c).expect("only cells inside are queued");
        if done[i] || g > cost[i] {
            continue;
        }
        if i == goal_i {
            let mut path = vec![c];
            let mut at = c;
            while at != start {
                at -= STEPS[from[index(at).expect("inside")] as usize];
                path.push(at);
            }
            path.reverse();
            return Some(path);
        }
        done[i] = true;
        expanded += 1;
        if expanded > budget {
            return None;
        }
        let here = get(c, i);
        let height = |c: IVec2, get: &mut dyn FnMut(IVec2, usize) -> Cell| match index(c) {
            Some(j) => get(c, j).height as f32,
            None => here.height as f32,
        };
        let gradient = Vec2::new(
            height(c + IVec2::X, &mut get) - height(c - IVec2::X, &mut get),
            height(c + IVec2::Y, &mut get) - height(c - IVec2::Y, &mut get),
        ) / (2 * CELL) as f32;
        let incoming = STEPS.get(from[i] as usize).map(|s| s.as_vec2().normalize());
        for (k, &step) in STEPS.iter().enumerate() {
            let next = c + step;
            let Some(j) = index(next) else { continue };
            if done[j] {
                continue;
            }
            // Never through a settlement's building, but to or from one's gate.
            if get(next, j).built && j != goal_i {
                continue;
            }
            // A knight's move passes between two cells; both must be dry land a trail
            // could cross.
            if step.abs().max_element() == 2 {
                let half = step.as_vec2() / 2.0;
                let between = [c + half.floor().as_ivec2(), c + half.ceil().as_ivec2()];
                if between.iter().any(|&b| index(b).is_none_or(|bi| get(b, bi).submerged() || (get(b, bi).built && bi != goal_i))) {
                    continue;
                }
            }
            let Some(mut step_cost) = step_cost(&here, &get(next, j), step, gradient) else { continue };
            // Turning back costs a little, so switchbacks have long legs, not zigzags.
            if incoming.is_some_and(|d| d.dot(step.as_vec2().normalize()) < -0.2) {
                step_cost += (TURN * UNIT) as u32;
            }
            let total = g.saturating_add(step_cost);
            if total < cost[j] {
                cost[j] = total;
                from[j] = k as u8;
                open.push(Reverse((total.saturating_add(estimate(next)), total, next.x, next.y)));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(_: IVec2) -> Cell {
        Cell { height: 140, water: 140, body: 0, built: false }
    }

    #[test]
    fn crosses_flat_land_straight() {
        let path = route(IVec2::ZERO, IVec2::new(20, 0), IVec2::splat(-10), IVec2::new(30, 10), 10_000, &flat).unwrap();
        assert_eq!(path.first(), Some(&IVec2::ZERO));
        assert_eq!(path.last(), Some(&IVec2::new(20, 0)));
        assert!(path.iter().all(|c| c.y == 0), "{path:?}");
    }

    #[test]
    fn switches_back_up_a_steep_slope() {
        // A slope rising 0.6 blocks per block northwards (+z): straight up is too steep.
        let slope = |c: IVec2| {
            let h = (140.0 + c.y as f32 * CELL as f32 * 0.6) as i16;
            Cell { height: h, water: h, body: 0, built: false }
        };
        let path = route(IVec2::ZERO, IVec2::new(0, 12), IVec2::new(-20, -2), IVec2::new(20, 14), 50_000, &slope).unwrap();
        // It wanders sideways to keep its grade down, turning back on long legs.
        let turns = path.windows(3).filter(|w| (w[1].x - w[0].x) * (w[2].x - w[1].x) < 0).count();
        assert!(path.iter().any(|c| c.x.abs() >= 6), "{path:?}");
        assert!((1..=3).contains(&turns), "{path:?}");
        let grade = |a: IVec2, b: IVec2| (slope(b).height - slope(a).height).abs() as f32 / ((b - a).as_vec2().length() * CELL as f32);
        let steep = path.windows(2).filter(|w| grade(w[0], w[1]) > HARD_GRADE).count();
        assert!(steep * 4 <= path.len(), "mostly gentle: {path:?}");
    }

    #[test]
    fn never_crosses_the_sea_and_gives_up_cleanly() {
        let sea = |c: IVec2| if c.x == 5 { Cell { height: 100, water: 128, body: crate::water::Body::Sea as u8, built: false } } else { flat(c) };
        assert_eq!(route(IVec2::ZERO, IVec2::new(10, 0), IVec2::new(-2, -2), IVec2::new(12, 2), 10_000, &sea), None);
        // Fords shallow river water.
        let creek = |c: IVec2| if c.x == 5 { Cell { height: 139, water: 140, body: crate::water::Body::Creek as u8, built: false } } else { flat(c) };
        assert!(route(IVec2::ZERO, IVec2::new(10, 0), IVec2::new(-2, -2), IVec2::new(12, 2), 10_000, &creek).is_some());
    }

    #[test]
    fn routes_are_deterministic() {
        let hills = |c: IVec2| {
            let h = (150.0 + 20.0 * ((c.x as f32 * 0.3).sin() + (c.y as f32 * 0.21).cos())) as i16;
            Cell { height: h, water: h, body: 0, built: false }
        };
        let a = route(IVec2::ZERO, IVec2::new(30, 25), IVec2::splat(-10), IVec2::splat(40), 50_000, &hills);
        assert!(a.is_some());
        assert_eq!(a, route(IVec2::ZERO, IVec2::new(30, 25), IVec2::splat(-10), IVec2::splat(40), 50_000, &hills));
    }
}
