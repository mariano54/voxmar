//! How enclosed the player is and what is around them, measured the way AmbientSounds
//! does: a capped flood fill from the eye through everything sound passes (air, plants,
//! leaves, water), which counts the air it reaches, how much of it is open to the sky, and
//! the blocks it meets, weighted by how near they are.
//!
//! A single column test (Resonance's) is fooled by any overhang and switches abruptly; the
//! fill instead gives continuous answers: a cave mouth is half open, a forest is open to the
//! sky under its leaves, a small room has little air. It runs on a worker thread over a
//! snapshot of the columns around the player, a few times a second at most, and costs a
//! millisecond or two.
//!
//! The fill also keeps a few random blocks of each group it met, nearer ones likelier, so
//! one-shots can sound from real places: birds from leaves that are actually there.
//!
//! Whether the player is indoors is asked apart, as the fill leaks out of an open door and
//! would call a house half outdoors: a roof over the columns around the eye, and walls met
//! by short rays out from it at head and chest height ([`Enclosure::shelter`]).

use std::collections::VecDeque;

use bevy::math::IVec3;
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::WORLD_HEIGHT;

use crate::sfx::mix;

/// Steps the fill goes from the eye.
const REACH: i32 = 24;
/// Air cells the fill visits at most.
const MAX_CELLS: usize = 16_000;
/// Air cells that count as entirely open (a flat field fills about this many).
const OPEN_AIR: f32 = 5_000.0;
/// Steps within which the sky and canopy are measured.
const NEAR: i32 = 8;
/// Above the highest blocking block or leaves of its column, the fill only climbs this
/// far: open sky needs no exploring.
const SKY_HEADROOM: i32 = 3;
/// Blocks above the eye the sky test looks; anything higher counts as open.
const SKY_LOOK: i32 = 64;
/// Sample blocks kept per group.
pub const SAMPLES: usize = 8;

const SIDE: i32 = 2 * REACH + 3;

/// What the scan found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Enclosure {
    /// Air reached, 0 (enclosed) to 1 (open land).
    pub air: f32,
    /// Share of the nearby air open to the sky.
    pub sky: f32,
    /// Share of the nearby sky-open air that has leaves above it.
    pub canopy: f32,
    /// How indoors the player is, 0..1: a roof overhead and walls around (a door may stand
    /// open). A cave counts; a tree's leaves do not.
    pub shelter: f32,
    /// Share of the columns around the eye with a roof close overhead that the sky falls on:
    /// rain is heard drumming on it, walls or not.
    pub roof: f32,
    /// Weighted count of each group's blocks met.
    pub groups: Vec<f32>,
    /// A few blocks of each group, nearer ones likelier.
    pub samples: Vec<Vec<IVec3>>,
    /// Whether each sample has nothing over it but the sky.
    pub open_to_sky: Vec<Vec<bool>>,
}

/// What the scan needs to know of each block id.
pub struct Kinds {
    /// Whether sound passes through it.
    pub open: Vec<bool>,
    /// Leaves: sound passes, and they make a canopy but do not hide the sky.
    pub leaves: Vec<bool>,
    pub group: Vec<Option<u8>>,
    /// A structure's core whose group counts it only while its fire burns.
    pub lit_only: Vec<bool>,
    /// Each block's wall group, if any (a home's walls by their shelter), counted besides
    /// its `group`, and never as the floor under a cell.
    pub wall_group: Vec<Option<u8>>,
    pub groups: usize,
    /// Blocks out from the eye a wall may be and still count, and above it a roof may be for
    /// rain to be heard on it.
    pub walls: i32,
    pub roof: i32,
    /// How far above a block the server looks for cover over it (survival.ron's
    /// `roof_scan`): the reach of [`Enclosure::open_to_sky`].
    pub roof_scan: i32,
}

/// How much a block `d` steps away counts.
fn weight(d: i32) -> f32 {
    match d {
        ..5 => 10.0,
        5..10 => 5.0,
        10..15 => 2.0,
        _ => 1.0,
    }
}

/// The highest block of a column that hides the sky, and the highest leaves above it.
#[derive(Clone, Copy)]
struct Roof {
    solid: i32,
    leaves: i32,
}

/// Floods from `eye` through blocks `kinds` marks open. `block` gives the block at a
/// position, `None` where the world is not loaded (treated as a wall); `burning` whether a
/// fire burns at a structure's core. `seed` makes the sample choice.
pub fn scan(eye: IVec3, block: impl Fn(IVec3) -> Option<BlockId>, burning: impl Fn(IVec3) -> bool, kinds: &Kinds, seed: u64) -> Enclosure {
    let origin = eye - IVec3::splat(SIDE / 2);
    let index = |p: IVec3| {
        let l = p - origin;
        ((l.y * SIDE + l.z) * SIDE + l.x) as usize
    };
    let inside = |p: IVec3| {
        let l = p - origin;
        l.min_element() >= 0 && l.max_element() < SIDE
    };
    let is_open = |b: Option<BlockId>| b.is_some_and(|b| kinds.open[b.0 as usize]);
    let mut seen = vec![false; (SIDE * SIDE * SIDE) as usize];
    let mut roofs: Vec<Option<Roof>> = vec![None; (SIDE * SIDE) as usize];
    let mut roof = |x: i32, z: i32| -> Roof {
        let slot = ((z - origin.z) * SIDE + (x - origin.x)) as usize;
        *roofs[slot].get_or_insert_with(|| {
            let mut leaves = i32::MIN;
            let top = (eye.y + SKY_LOOK).min(WORLD_HEIGHT - 1);
            for y in (eye.y - REACH - 1..=top).rev() {
                match block(IVec3::new(x, y, z)) {
                    Some(b) if kinds.leaves[b.0 as usize] => leaves = leaves.max(y),
                    Some(b) if kinds.open[b.0 as usize] => {}
                    // Unloaded columns are walls to the fill but say nothing of the sky.
                    None if y > eye.y => {}
                    _ => return Roof { solid: y, leaves },
                }
            }
            Roof { solid: i32::MIN, leaves }
        })
    };

    let mut found = Enclosure { groups: vec![0.0; kinds.groups], samples: vec![Vec::new(); kinds.groups], open_to_sky: vec![Vec::new(); kinds.groups], ..Default::default() };
    // Weighted reservoir per group: the samples with the largest keys u^(1 / weight).
    let mut keys: Vec<Vec<f32>> = vec![Vec::new(); kinds.groups];
    let mut random = seed;
    // Nothing over it but the sky, as the server judges it (`roof_scan`): no solid block,
    // leaves counting, within that many above; space not loaded says nothing.
    let open_to_sky = |p: IVec3| (1..=kinds.roof_scan).all(|dy| block(p + IVec3::Y * dy).is_none_or(|b| kinds.open[b.0 as usize] && !kinds.leaves[b.0 as usize]));
    let mut count = |found: &mut Enclosure, p: IVec3, group: u8, d: i32| {
        let (group, w) = (group as usize, weight(d));
        found.groups[group] += w;
        random = mix(random);
        let key = ((random >> 40) as f32 / (1u64 << 24) as f32).max(1e-9).powf(1.0 / w);
        let (samples, keys) = (&mut found.samples[group], &mut keys[group]);
        if samples.len() < SAMPLES {
            samples.push(p);
            keys.push(key);
            found.open_to_sky[group].push(open_to_sky(p));
        } else if let Some((i, &lowest)) = keys.iter().enumerate().min_by(|a, b| a.1.total_cmp(b.1))
            && key > lowest
        {
            samples[i] = p;
            keys[i] = key;
            found.open_to_sky[group][i] = open_to_sky(p);
        }
    };
    let mut meet = |found: &mut Enclosure, p: IVec3, b: BlockId, d: i32, floor: bool| {
        let wall = kinds.wall_group[b.0 as usize].filter(|_| !floor);
        let own = kinds.group[b.0 as usize].filter(|_| !kinds.lit_only[b.0 as usize] || burning(p));
        for group in [own, wall].into_iter().flatten() {
            count(found, p, group, d);
        }
    };

    let (mut cells, mut near, mut near_sky, mut near_canopy) = (0usize, 0u32, 0u32, 0u32);
    let mut queue = VecDeque::new();
    if is_open(block(eye)) {
        seen[index(eye)] = true;
        queue.push_back((eye, 0));
    }
    while let Some((p, d)) = queue.pop_front() {
        cells += 1;
        let r = roof(p.x, p.z);
        if d <= NEAR {
            near += 1;
            if p.y > r.solid {
                near_sky += 1;
                near_canopy += (r.leaves > p.y) as u32;
            }
        }
        if let Some(b) = block(p).filter(|b| !b.is_air()) {
            meet(&mut found, p, b, d, false);
        }
        for step in [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z, IVec3::Y, IVec3::NEG_Y] {
            let n = p + step;
            if !inside(n) || seen[index(n)] {
                continue;
            }
            seen[index(n)] = true;
            let b = block(n);
            if !is_open(b) {
                if let Some(b) = b {
                    meet(&mut found, n, b, d + 1, step == IVec3::NEG_Y);
                }
                continue;
            }
            if d >= REACH || cells + queue.len() >= MAX_CELLS {
                continue;
            }
            let r = roof(n.x, n.z);
            if n.y <= r.solid.max(r.leaves).max(eye.y) + SKY_HEADROOM {
                queue.push_back((n, d + 1));
            }
        }
    }
    found.air = (cells as f32 / OPEN_AIR).min(1.0);
    if near > 0 {
        found.sky = near_sky as f32 / near as f32;
    }
    if near_sky > 0 {
        found.canopy = near_canopy as f32 / near_sky as f32;
    }

    // Indoors: a roof over the eye's column and its neighbours, and walls on most sides.
    let (mut covered, mut roofed) = (0, 0);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let r = roof(eye.x + dx, eye.z + dz);
            covered += (r.solid > eye.y) as u32;
            roofed += (r.solid > eye.y && r.solid <= eye.y + kinds.roof) as u32;
        }
    }
    let mut walls = 0;
    let directions = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];
    for (dx, dz) in directions {
        for height in [0, -1] {
            let wall = (1..=kinds.walls).map(|step| block(eye + IVec3::new(dx * step, height, dz * step))).find(|b| !is_open(*b));
            // Unloaded space is no wall here: it says nothing of what is built.
            walls += wall.is_some_and(|b| b.is_some()) as u32;
        }
    }
    let walls = walls as f32 / (2 * directions.len()) as f32;
    // A doorway or a window or two still leaves a room indoors; a wall or two is a porch.
    let t = ((walls - 0.35) / 0.45).clamp(0.0, 1.0);
    found.shelter = covered as f32 / 9.0 * t * t * (3.0 - 2.0 * t);
    found.roof = roofed as f32 / 9.0;
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const AIR: BlockId = BlockId(0);
    const STONE: BlockId = BlockId(1);
    const LEAVES: BlockId = BlockId(2);
    const WATER: BlockId = BlockId(3);
    const GRASS: BlockId = BlockId(4);

    fn kinds() -> Kinds {
        Kinds {
            open: vec![true, false, true, true, false],
            leaves: vec![false, false, true, false, false],
            // Groups: 0 rock, 1 leaves, 2 water, 3 grass.
            group: vec![None, Some(0), Some(1), Some(2), Some(3)],
            lit_only: vec![false; 5],
            wall_group: vec![None; 5],
            groups: 4,
            walls: 6,
            roof: 8,
            roof_scan: 48,
        }
    }

    /// A block heard only while lit (a hearthstone, say) is not counted cold, and counted
    /// where its fire burns.
    #[test]
    fn a_cold_core_is_not_heard() {
        let mut kinds = kinds();
        kinds.lit_only[STONE.0 as usize] = true;
        let room = |p: IVec3| Some(if p.max_element() > 2 || p.min_element() < -2 { STONE } else { AIR });
        let cold = scan(IVec3::ZERO, room, |_| false, &kinds, 1);
        let lit = scan(IVec3::ZERO, room, |p| p.x == 3, &kinds, 1);
        assert_eq!(cold.groups[0], 0.0, "a cold core was heard");
        assert!(lit.groups[0] > 0.0 && lit.samples[0].iter().all(|p| p.x == 3), "only the burning cores are heard: {:?}", lit.samples[0]);
    }

    fn field(p: IVec3) -> Option<BlockId> {
        Some(match p.y {
            ..63 => STONE,
            63 => GRASS,
            _ => AIR,
        })
    }

    #[test]
    fn open_field_is_open_to_the_sky() {
        let found = scan(IVec3::new(0, 65, 0), field, |_| true, &kinds(), 1);
        assert!(found.air > 0.9, "{}", found.air);
        assert_eq!(found.sky, 1.0);
        assert_eq!(found.canopy, 0.0);
        assert!(found.groups[3] > 100.0, "grass all around");
        assert_eq!(found.groups[0], 0.0, "the stone is under the grass");
        assert!(found.samples[3].len() == SAMPLES && found.samples[3].iter().all(|p| p.y == 63));
        assert_eq!((found.shelter, found.roof), (0.0, 0.0));
        assert!(found.open_to_sky[3].iter().all(|&sky| sky), "the field's grass is under the sky");
    }

    /// A block with nothing over it is open to the sky; one under a roof or a canopy is not
    /// (leaves are cover, as the server judges it), wherever the listener stands.
    #[test]
    fn a_block_under_a_roof_is_not_open_to_the_sky() {
        // Grass on stone; a stone slab on posts over the grass where x is 2 to 6, and a
        // tree's crown high over it where x is -6 to -3.
        let porch = |p: IVec3| {
            Some(match (p.x, p.y) {
                (_, ..63) => STONE,
                (_, 63) => GRASS,
                (2..=6, 67) => STONE,
                (-6..=-3, 80) => LEAVES,
                _ => AIR,
            })
        };
        let covered = |x: i32| (2..=6).contains(&x) || (-6..=-3).contains(&x);
        let found = scan(IVec3::new(0, 65, 0), porch, |_| true, &kinds(), 1);
        let grass: Vec<_> = found.samples[3].iter().zip(&found.open_to_sky[3]).collect();
        assert!(grass.iter().any(|(p, _)| covered(p.x)) && grass.iter().any(|(p, _)| !covered(p.x)), "{grass:?}");
        assert!(grass.iter().all(|(p, sky)| **sky != covered(p.x)), "{grass:?}");
    }

    /// A 7×7 hut of stone on the field, three high inside, roofed at 67.
    fn hut(door: bool, window: bool) -> impl Fn(IVec3) -> Option<BlockId> {
        move |p: IVec3| {
            let (edge, inside) = (p.x.abs() == 3 || p.z.abs() == 3, p.x.abs() <= 3 && p.z.abs() <= 3);
            Some(if inside && p.y == 67 {
                STONE
            } else if inside && edge && (64..67).contains(&p.y) {
                let doorway = door && p.x == 3 && p.z == 0 && p.y < 66;
                let pane = window && p.x == -3 && p.z.abs() <= 1 && p.y == 65;
                if doorway || pane { AIR } else { STONE }
            } else {
                field(p).unwrap()
            })
        }
    }

    #[test]
    fn a_hut_is_indoors_with_its_door_open() {
        let eye = IVec3::new(0, 65, 0);
        let shut = scan(eye, hut(false, false), |_| false, &kinds(), 1);
        assert_eq!((shut.shelter, shut.roof), (1.0, 1.0));
        let open = scan(eye, hut(true, true), |_| false, &kinds(), 1);
        assert!(open.shelter > 0.85, "{}", open.shelter);
        assert_eq!(open.roof, 1.0);
        // The fill leaks out of the door, so by sky and air alone it is half outdoors.
        assert!(open.air > shut.air && open.sky > 0.0, "{} {}", open.air, open.sky);
        // Standing in the doorway is on the way out; a step outside is outdoors.
        let doorway = scan(IVec3::new(3, 65, 0), hut(true, true), |_| false, &kinds(), 1);
        assert!(doorway.shelter < open.shelter, "{}", doorway.shelter);
        let outside = scan(IVec3::new(5, 65, 0), hut(true, true), |_| false, &kinds(), 1);
        assert_eq!((outside.shelter, outside.roof), (0.0, 0.0));
    }

    #[test]
    fn a_roof_on_posts_is_heard_in_the_rain_but_is_not_indoors() {
        let porch = |p: IVec3| Some(if p.x.abs() <= 3 && p.z.abs() <= 3 && p.y == 67 { STONE } else { field(p).unwrap() });
        let found = scan(IVec3::new(0, 65, 0), porch, |_| false, &kinds(), 1);
        assert_eq!((found.shelter, found.roof), (0.0, 1.0));
    }

    #[test]
    fn a_closed_room_is_small_and_dark() {
        // A 5×3×5 room cut into stone.
        let room = |p: IVec3| Some(if p.x.abs() <= 2 && p.z.abs() <= 2 && (64..67).contains(&p.y) { AIR } else { STONE });
        let found = scan(IVec3::new(0, 65, 0), room, |_| true, &kinds(), 1);
        assert!(found.air < 0.02, "{}", found.air);
        assert_eq!(found.sky, 0.0);
        assert!(found.groups[0] > 300.0, "close rock all around: {}", found.groups[0]);
        // Underground is sheltered, but no rain falls on the rock far above.
        assert_eq!((found.shelter, found.roof), (1.0, 0.0));
    }

    #[test]
    fn a_cave_mouth_is_partly_open() {
        // A tunnel running into a hillside along +x from open ground at x < 0.
        let hill = |p: IVec3| {
            Some(if p.x < 0 {
                field(p).unwrap()
            } else if p.y >= 64 && p.y < 67 && p.z.abs() <= 1 {
                AIR
            } else {
                STONE
            })
        };
        let found = scan(IVec3::new(2, 65, 0), hill, |_| true, &kinds(), 1);
        assert!(found.sky > 0.2 && found.sky < 0.9, "{}", found.sky);
    }

    #[test]
    fn under_a_canopy_is_still_outdoors() {
        let forest = |p: IVec3| Some(if (70..74).contains(&p.y) { LEAVES } else { field(p).unwrap() });
        let found = scan(IVec3::new(0, 65, 0), forest, |_| true, &kinds(), 1);
        assert_eq!(found.sky, 1.0);
        assert!(found.canopy > 0.9, "{}", found.canopy);
        assert!(found.groups[1] > 100.0);
        assert!(found.samples[1].iter().all(|p| (70..74).contains(&p.y)));
        assert_eq!((found.shelter, found.roof), (0.0, 0.0), "leaves are no roof");
    }

    #[test]
    fn water_is_heard_through_and_counted() {
        let shore = |p: IVec3| Some(if p.x > 3 && p.y == 64 { WATER } else { field(p).unwrap() });
        let found = scan(IVec3::new(0, 65, 0), shore, |_| true, &kinds(), 1);
        assert!(found.groups[2] > 50.0);
        assert!(found.samples[2].iter().all(|p| p.x > 3));
    }

    #[test]
    fn unloaded_space_is_a_wall() {
        let found = scan(IVec3::new(0, 65, 0), |p| (p.x < 4).then(|| field(p).unwrap()), |_| true, &kinds(), 1);
        assert!(found.air > 0.2 && found.air < 0.9);
    }
}
