//! Finding the notable places water makes: a lake high in the hills, a tarn, a waterfall, a
//! confluence, a river's mouth, a tunnel, an island, a hot spring, a gorge. Tools and tests use it
//! to look at them without knowing where a seed put them, and gameplay can use it to point
//! players at them.

use voxmar_engine::glam::{IVec2, IVec3, Vec3Swizzles};

use crate::geology::GeoSample;
use crate::{Body, Overworld, SEA_LEVEL};

/// A kind of place water makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WaterSight {
    /// A lake at least 20 blocks above the sea.
    HighlandLake,
    /// The tallest drop in a creek or river.
    Waterfall,
    /// Three or more arms of creek or river meeting.
    Confluence,
    /// A river or creek meeting the sea.
    RiverMouth,
    /// A river running under the ground.
    RiverTunnel,
    /// Ground inside a river, with river on both sides.
    RiverIsland,
    HotSpring,
    /// A creek or river between walls at least 6 blocks high on both sides, within 6
    /// blocks of it.
    Gorge,
    /// A still lake as high as tarns lie (`content::TarnsDef::floor`): a tarn, or a lake
    /// among the peaks.
    Tarn,
}

impl WaterSight {
    pub const ALL: [Self; 9] =
        [Self::HighlandLake, Self::Waterfall, Self::Confluence, Self::RiverMouth, Self::RiverTunnel, Self::RiverIsland, Self::HotSpring, Self::Gorge, Self::Tarn];

    pub fn name(self) -> &'static str {
        match self {
            Self::HighlandLake => "highland lake",
            Self::Waterfall => "waterfall",
            Self::Confluence => "confluence",
            Self::RiverMouth => "river mouth",
            Self::RiverTunnel => "river tunnel",
            Self::RiverIsland => "river island",
            Self::HotSpring => "hot spring",
            Self::Gorge => "gorge",
            Self::Tarn => "tarn",
        }
    }
}

impl Overworld {
    /// The nearest place within `radius` blocks of `centre` where a creek or river runs
    /// deep in a slot gorge: the column, and the height of its top water block. Much
    /// quicker over a wide area than `water_sights`, as it reads the rivers rather than the
    /// ground.
    /// The walls must stand `GORGE_WALLS` blocks over the water on both sides, within 8
    /// blocks of it, across it straight or diagonally.
    pub fn locate_gorge(&self, centre: IVec2, radius: i32) -> Option<IVec3> {
        const GORGE_WALLS: i32 = 8;
        let walls = |at: IVec3| {
            let side = |dx: i32, dz: i32| {
                (3..=8).map(|k| self.geo(at.x + dx * k, at.z + dz * k)).filter(|g| !g.submerged()).map(|g| g.height).max().unwrap_or(at.y) - at.y
            };
            [(1, 0), (0, 1), (1, 1), (1, -1)].map(|(dx, dz)| side(dx, dz).min(side(-dx, -dz))).into_iter().max().unwrap_or(0)
        };
        self.rivers.gorges_near(self.stages(), centre, radius, 24).into_iter().find(|&at| walls(at) >= GORGE_WALLS)
    }

    /// The nearest visible pool made by the tarn rule, within `radius` blocks of
    /// `centre`. Ordinary high lakes do not stand in for a tarn. Twenty surface columns
    /// distinguish a pool that can be seen from a puddle; generation itself is unchanged.
    pub fn locate_tarn(&self, centre: IVec2, radius: i32) -> Option<IVec3> {
        if radius < 0 || self.rivers.tarns.is_none() {
            return None;
        }
        for pit in self.rivers.tarns_near(self.stages(), centre, radius) {
            let mut wet = Vec::new();
            for z in pit.z - 8..=pit.z + 8 {
                for x in pit.x - 8..=pit.x + 8 {
                    let g = self.geo(x, z);
                    if g.body == Body::Lake && g.submerged() && g.water == pit.y {
                        wet.push(IVec3::new(x, g.water, z));
                    }
                }
            }
            if wet.len() >= 20 {
                return wet.into_iter().min_by_key(|p| ((p.xz() - pit.xz()).length_squared(), p.x, p.z));
            }
        }
        None
    }

    /// The best place of each kind within `range` blocks of `centre` (searched every two
    /// blocks), each as the column and the height of its water, preferring nearer ones.
    /// Kinds none of which lie within range are left out. Costs about a second for a
    /// range of 400.
    pub fn water_sights(&self, centre: IVec2, range: i32) -> Vec<(WaterSight, IVec3)> {
        const STEP: i32 = 2;
        let n = 2 * range / STEP + 1;
        let geos: Vec<GeoSample> = (0..n * n).map(|i| self.geo(centre.x - range + i % n * STEP, centre.y - range + i / n * STEP)).collect();
        let at = |i: i32, j: i32| geos[(j.clamp(0, n - 1) * n + i.clamp(0, n - 1)) as usize];
        let mut best: [Option<(f32, IVec3)>; WaterSight::ALL.len()] = [None; WaterSight::ALL.len()];
        let tarns = self.rivers.tarn_floor();
        for j in 1..n - 1 {
            for i in 1..n - 1 {
                let g = at(i, j);
                let p = IVec2::new(centre.x - range + i * STEP, centre.y - range + j * STEP);
                let near = 1.0 / (1.0 + (p - centre).as_vec2().length() / 200.0);
                let mut consider = |sight: WaterSight, score: f32| {
                    let score = score * near;
                    let slot = &mut best[sight as usize];
                    if slot.is_none_or(|(s, _)| score > s) {
                        *slot = Some((score, IVec3::new(p.x, g.water, p.y)));
                    }
                };
                if g.cavity.is_some() {
                    consider(WaterSight::RiverTunnel, 1.0);
                }
                let river = |o: GeoSample| o.submerged() && o.body == Body::River;
                if !g.submerged() && g.body == Body::River && (river(at(i - 2, j)) && river(at(i + 2, j)) || river(at(i, j - 2)) && river(at(i, j + 2))) {
                    consider(WaterSight::RiverIsland, 1.0);
                }
                if !g.submerged() {
                    continue;
                }
                if g.body == Body::Spring {
                    consider(WaterSight::HotSpring, 1.0);
                }
                if g.body == Body::Lake && g.water > SEA_LEVEL + 20 {
                    consider(WaterSight::HighlandLake, (g.water - SEA_LEVEL) as f32);
                }
                if g.body == Body::Lake && tarns.is_some_and(|floor| g.water as f32 >= floor) {
                    consider(WaterSight::Tarn, 1.0);
                }
                // From the top of the drop.
                let drop = [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|(di, dj)| at(i + di, j + dj)).iter().filter(|o| o.submerged()).map(|o| g.water - o.water).max();
                if let Some(drop) = drop.filter(|&d| d >= 3)
                    && g.body != Body::Lake
                {
                    consider(WaterSight::Waterfall, (drop * drop) as f32);
                }
                let channel = |o: GeoSample| o.submerged() && matches!(o.body, Body::Creek | Body::River);
                if channel(g) {
                    // Walls on both sides, across the water one way or the other.
                    let wall = |o: GeoSample| if o.submerged() { 0 } else { o.height - g.water };
                    let across = [(3, 0), (0, 3)].map(|(di, dj)| wall(at(i + di, j + dj)).min(wall(at(i - di, j - dj)))).into_iter().max().unwrap_or(0);
                    if across >= 6 {
                        consider(WaterSight::Gorge, across as f32);
                    }
                    // Arms of water crossing a ring around the point.
                    let ring: Vec<bool> = (0..32)
                        .map(|k| {
                            let a = k as f32 / 32.0 * std::f32::consts::TAU;
                            channel(at(i + (a.cos() * 5.0).round() as i32, j + (a.sin() * 5.0).round() as i32))
                        })
                        .collect();
                    let arms = (0..32).filter(|&k| ring[k] && !ring[(k + 31) % 32]).count();
                    if arms >= 3 && ring.iter().filter(|&&w| w).count() < 16 {
                        consider(WaterSight::Confluence, arms as f32 + if g.body == Body::River { 2.0 } else { 0.0 });
                    }
                    let sea = (-6..=6)
                        .flat_map(|dj| (-6..=6).map(move |di| (di, dj)))
                        .filter(|&(di, dj)| {
                            let o = at(i + di, j + dj);
                            o.body == Body::Sea && o.submerged()
                        })
                        .count();
                    if g.water == SEA_LEVEL && sea > 0 {
                        consider(WaterSight::RiverMouth, sea as f32 + if g.body == Body::River { 20.0 } else { 0.0 });
                    }
                }
            }
        }
        WaterSight::ALL.into_iter().zip(best).filter_map(|(sight, found)| found.map(|(_, at)| (sight, at))).collect()
    }
}
