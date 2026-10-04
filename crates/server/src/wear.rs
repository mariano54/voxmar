//! Ground worn by feet.
//!
//! Every step a player takes onto a new block wears the ground under it, and a little to
//! either side and ahead, so paths form about three blocks wide, not one. Plants go first:
//! tall grass is trampled to short grass, short grass and flowers to nothing. Then the
//! ground itself passes through stages (`wear.ron`: grass to worn grass to packed earth
//! to coarse dirt), each after a number of steps drawn for that block and stage from a
//! hash, so paths fray irregularly at their edges, the same in every world with the seed.
//!
//! The steps are kept per surface column in the column itself (`Column::wear`), so they
//! save and load with it; only the blocks they change are sent, and only to the players
//! that have the column. Unused ground heals, a stage at a time and lazily, when a saved
//! column is loaded: a stage every few days, twice as fast for a lone patch with no worn
//! ground beside it, so stray footprints vanish while real paths last. Healing undoes only
//! what feet did: ground that was generated worn, like a trail's tread, stays.

use rustc_hash::FxHashMap;
use serde::Deserialize;
use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind};
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos, WORLD_HEIGHT};
use voxmar_engine::content::ContentFile;
use voxmar_engine::glam::IVec3;
use voxmar_engine::schedule::Tier;
use voxmar_engine::world::Column;
use voxmar_protocol::{PlayerId, TICKS_PER_DAY};

use crate::Server;
use crate::process::Process;

/// The file wear is defined in, in any content package.
pub const FILE: &str = "wear.ron";

/// Steps are counted in quarters.
const QUARTERS: f32 = 4.0;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WearFile {
    #[serde(default)]
    stages: Vec<StageDef>,
    #[serde(default)]
    trample: Vec<TrampleDef>,
    heal_days: Option<f32>,
    tuft: Option<(String, f32)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StageDef {
    from: String,
    to: String,
    steps: (f32, f32),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrampleDef {
    /// A block, or `#tag` for every block with the tag.
    from: String,
    to: String,
    /// The upper half of a tall plant, cleared with it.
    #[serde(default)]
    top: Option<String>,
    steps: (f32, f32),
}

#[derive(Clone, Copy, Debug)]
struct Stage {
    to: BlockId,
    /// Steps (in quarters) the ground takes to wear to the next stage, drawn per block
    /// between these.
    steps: (u8, u8),
}

#[derive(Clone, Copy, Debug)]
struct Trample {
    to: BlockId,
    top: Option<BlockId>,
    steps: (u8, u8),
}

/// How ground and plants wear, resolved against the blocks.
#[derive(Debug)]
pub struct WearRules {
    /// The next stage of each block that wears, by block id.
    stages: Vec<Option<Stage>>,
    /// The stage each worn block heals back to, by block id.
    heals: Vec<Option<BlockId>>,
    trample: Vec<Option<Trample>>,
    /// Days a stage takes to heal.
    heal_days: f32,
    /// A plant that may sprout on ground healing back to grass, and its chance.
    tuft: Option<(BlockId, f32)>,
    seed: u64,
}

impl WearRules {
    /// Reads every package's `wear.ron`, in package order; a later package's settings
    /// replace an earlier one's, and its stages and trampling add to them.
    pub fn load(files: &[ContentFile], registry: &BlockRegistry, seed: u64) -> Result<Self, String> {
        let n = registry.len();
        let mut rules = WearRules { stages: vec![None; n], heals: vec![None; n], trample: vec![None; n], heal_days: 5.0, tuft: None, seed };
        for file in files.iter().filter(|f| f.name == FILE) {
            let at = |e: &dyn std::fmt::Display| format!("{}/{FILE}: {e}", file.package);
            let def: WearFile = ron::Options::default()
                .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
                .from_str(&file.source)
                .map_err(|e| at(&e))?;
            let block = |name: &str| {
                let name = if name.contains(':') { name.to_owned() } else { format!("{}:{name}", file.package) };
                registry.id(&name).ok_or_else(|| at(&format!("unknown block `{name}`")))
            };
            let quarters = |(lo, hi): (f32, f32)| -> Result<(u8, u8), String> {
                if !(lo > 0.0 && lo <= hi && hi * QUARTERS <= 255.0) {
                    return Err(at(&format!("steps ({lo}, {hi}) must be positive, in order and at most 63")));
                }
                Ok(((lo * QUARTERS).round() as u8, (hi * QUARTERS).round() as u8))
            };
            for stage in def.stages {
                let (from, to) = (block(&stage.from)?, block(&stage.to)?);
                rules.stages[from.0 as usize] = Some(Stage { to, steps: quarters(stage.steps)? });
                // Where several stages wear into the same block, the first says what it heals
                // back to (grass grows back over trodden dirt too).
                rules.heals[to.0 as usize].get_or_insert(from);
            }
            for trample in def.trample {
                let to = block(&trample.to)?;
                let top = trample.top.as_deref().map(block).transpose()?;
                let rule = Trample { to, top, steps: quarters(trample.steps)? };
                // Only plants are trampled, whatever else shares their tag.
                let from: Vec<BlockId> = match trample.from.strip_prefix('#') {
                    Some(tag) => registry.tagged(tag).filter(|&b| registry.info(b).render == RenderKind::Plant).collect(),
                    None => vec![block(&trample.from)?],
                };
                if from.is_empty() {
                    return Err(at(&format!("no block has the tag `{}`", &trample.from[1..])));
                }
                for block in from {
                    rules.trample[block.0 as usize] = Some(rule);
                }
            }
            if let Some(days) = def.heal_days {
                rules.heal_days = days.max(0.01);
            }
            if let Some((name, chance)) = def.tuft {
                rules.tuft = Some((block(&name)?, chance));
            }
        }
        Ok(rules)
    }

    /// The steps `(lo, hi)` drawn for the block at `pos`, for the purpose `salt`.
    fn threshold(&self, (lo, hi): (u8, u8), pos: IVec3, salt: u64) -> u8 {
        let u = (hash(self.seed ^ salt, pos) >> 40) as f32 / (1u64 << 24) as f32;
        lo + ((hi - lo) as f32 * u).round() as u8
    }

    /// Heals `column` (with its origin) for the days up to `today`: each worn spot steps
    /// back a stage per `heal_days` since it was last walked on (half that alone), never
    /// below the ground it was generated with. Spent footsteps are forgotten.
    pub fn heal(&self, column: &mut Column, origin: IVec3, today: u32, registry: &BlockRegistry) {
        let worn: Vec<u16> = column.wear.iter().filter(|w| w.stages > 0).map(|w| w.at).collect();
        let mut wear = std::mem::take(&mut column.wear);
        for spot in &mut wear {
            let elapsed = today.saturating_sub(spot.day) as f32;
            let (x, z) = ((spot.at as i32) % CHUNK_SIZE, (spot.at as i32) / CHUNK_SIZE);
            let alone = ![(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dz)| {
                let (nx, nz) = (x + dx, z + dz);
                (0..CHUNK_SIZE).contains(&nx) && (0..CHUNK_SIZE).contains(&nz) && worn.contains(&((nz * CHUNK_SIZE + nx) as u16))
            });
            let period = if alone { self.heal_days / 2.0 } else { self.heal_days };
            if elapsed < period {
                continue;
            }
            spot.steps = 0;
            let stages = ((elapsed / period) as u32).min(spot.stages as u32) as u8;
            if stages == 0 {
                continue;
            }
            // The ground: the highest block that is not air or a plant.
            let Some(y) = (0..WORLD_HEIGHT).rev().find(|&y| {
                let block = column.block(IVec3::new(x, y, z));
                !block.is_air() && registry.info(block).render != RenderKind::Plant
            }) else {
                continue;
            };
            let at = IVec3::new(x, y, z);
            let mut block = column.block(at);
            for _ in 0..stages {
                let Some(back) = self.heals[block.0 as usize] else { break };
                block = back;
                spot.stages -= 1;
            }
            column.set_block(at, block);
            spot.day += (stages as f32 * period) as u32;
            // Grass coming back now and then sprouts a tuft.
            if let Some((tuft, chance)) = self.tuft
                && self.heals[block.0 as usize].is_none()
                && self.stages[block.0 as usize].is_some()
                && y + 1 < WORLD_HEIGHT
                && column.block(at + IVec3::Y).is_air()
                && ((hash(self.seed ^ 0x7f7, origin + at) >> 40) as f32 / (1u64 << 24) as f32) < chance
            {
                column.set_block(at + IVec3::Y, tuft);
            }
        }
        wear.retain(|w| w.steps > 0 || w.stages > 0);
        column.wear = wear;
    }
}

fn hash(seed: u64, p: IVec3) -> u64 {
    let mut z = seed ^ (p.x as u32 as u64) ^ ((p.z as u32 as u64) << 32) ^ (p.y as u32 as u64).wrapping_mul(0xff51_afd7_ed55_8ccd);
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The day number of a time of day in ticks.
pub fn day(ticks: f64) -> u32 {
    (ticks / TICKS_PER_DAY as f64) as u32
}

/// Footsteps: wear under each player who stepped onto a new block this tick.
#[derive(Default)]
pub(crate) struct Footsteps {
    last: FxHashMap<PlayerId, IVec3>,
}

impl Process for Footsteps {
    fn tier(&self) -> Tier {
        Tier::High
    }

    fn run(&mut self, server: &mut Server) {
        let mut steps = Vec::new();
        self.last.retain(|id, _| server.players.contains_key(id));
        for player in server.players.values() {
            if !player.joined || !player.health.alive() || player.flying {
                continue;
            }
            let ground = (player.position - voxmar_engine::glam::Vec3::Y * 0.05).floor().as_ivec3();
            if !server.is_solid(ground) || self.last.insert(player.id, ground) == Some(ground) {
                continue;
            }
            // Facing, to the nearest axis.
            let facing = voxmar_engine::glam::Vec2::new(-player.yaw.sin(), -player.yaw.cos());
            let ahead = if facing.x.abs() > facing.y.abs() { IVec3::new(facing.x.signum() as i32, 0, 0) } else { IVec3::new(0, 0, facing.y.signum() as i32) };
            let side = IVec3::new(-ahead.z, 0, ahead.x);
            steps.push((ground, 4u8));
            for (offset, quarters) in [(side, 2u8), (-side, 2), (ahead, 1)] {
                if let Some(ground) = server.ground_near(ground + offset) {
                    steps.push((ground, quarters));
                }
            }
        }
        for (ground, quarters) in steps {
            server.wear(ground, quarters);
        }
    }
}

impl Server {
    /// The ground of the column at `pos` within a block up or down: its highest solid
    /// block with something walkable above.
    fn ground_near(&self, pos: IVec3) -> Option<IVec3> {
        [1, 0, -1].into_iter().map(|dy| pos + IVec3::Y * dy).find(|&p| self.is_solid(p) && !self.is_solid(p + IVec3::Y))
    }

    /// Wears the ground at `ground` (and any plant on it) by `quarters` of a step.
    fn wear(&mut self, ground: IVec3, quarters: u8) {
        let rules = self.wear_rules.clone();
        let (Some(below), Some(above)) = (self.block(ground), self.block(ground + IVec3::Y)) else { return };
        let stage = rules.stages[below.0 as usize];
        let trample = rules.trample[above.0 as usize];
        let water = self.content.blocks.id("base:water");
        if stage.is_none() && trample.is_none() {
            return;
        }
        let today = day(self.day);
        let column = ColumnPos::of_block(ground);
        let local = ground - column.origin();
        let Some(data) = self.world.column_mut(column) else { return };
        let spot = data.wear_mut(local.x, local.z);
        spot.steps = spot.steps.saturating_add(quarters);
        spot.day = today;
        let mut changes = Vec::new();
        if let Some(rule) = trample {
            let needed = rules.threshold(rule.steps, ground, 0x7a3b);
            if spot.steps >= needed {
                spot.steps -= needed;
                changes.push((ground + IVec3::Y, rule.to));
                if let Some(top) = rule.top
                    && self.world.block(ground + IVec3::Y * 2) == Some(top)
                {
                    changes.push((ground + IVec3::Y * 2, BlockId::AIR));
                }
            }
        } else if let Some(rule) = stage
            && Some(above) != water
        {
            let needed = rules.threshold(rule.steps, ground, below.0 as u64);
            if spot.steps >= needed {
                spot.steps = 0;
                spot.stages = spot.stages.saturating_add(1);
                changes.push((ground, rule.to));
            }
        }
        let spot = *self.world.column(column).and_then(|c| c.wear_at(local.x, local.z)).expect("just worn");
        if spot.steps == 0 && spot.stages == 0 && let Some(data) = self.world.column_mut(column) {
            data.wear.retain(|w| w.at != spot.at);
        }
        if self.save.is_some() {
            self.dirty.insert(column);
        }
        for (pos, block) in changes {
            self.set_block(pos, block);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> std::sync::Arc<BlockRegistry> {
        let source = include_str!("../../../assets/content/base/blocks.ron");
        std::sync::Arc::new(BlockRegistry::from_ron("base", source).unwrap())
    }

    fn rules(registry: &BlockRegistry) -> WearRules {
        let source = include_str!("../../../assets/content/base/wear.ron");
        WearRules::load(&[ContentFile { package: "base".into(), name: FILE.into(), source: source.into() }], registry, 1).unwrap()
    }

    #[test]
    fn wear_rules_load_and_chain() {
        let registry = registry();
        let rules = rules(&registry);
        let id = |name: &str| registry.expect(&format!("base:{name}"));
        let next = |name: &str| rules.stages[id(name).0 as usize].map(|s| s.to);
        assert_eq!(next("grass_block"), Some(id("worn_grass_block")));
        assert_eq!(next("worn_grass_block"), Some(id("packed_earth")));
        assert_eq!(next("packed_earth"), Some(id("coarse_dirt")));
        assert_eq!(rules.heals[id("coarse_dirt").0 as usize], Some(id("packed_earth")));
        assert_eq!(rules.trample[id("tall_grass").0 as usize].map(|t| t.to), Some(id("short_grass")));
        assert!(rules.trample[id("poppy").0 as usize].is_some(), "flowers by tag");
        // Thresholds vary by block, within their range, and are the same every time.
        let range = rules.stages[id("grass_block").0 as usize].unwrap().steps;
        let draws: Vec<u8> = (0..50).map(|x| rules.threshold(range, IVec3::new(x, 64, 0), 3)).collect();
        assert!(draws.iter().all(|&d| (range.0..=range.1).contains(&d)));
        assert!(draws.iter().any(|&d| d != draws[0]));
        assert_eq!(draws[7], rules.threshold(range, IVec3::new(7, 64, 0), 3));
    }

    #[test]
    fn healing_steps_back_a_stage_at_a_time_and_faster_alone() {
        let registry = registry();
        let rules = rules(&registry);
        let id = |name: &str| registry.expect(&format!("base:{name}"));
        let mut column = Column::default();
        // Two worn spots side by side at (0, 0) and (1, 0), and one alone at (10, 10),
        // all worn two stages to packed earth on day 0.
        for (x, z) in [(0, 0), (1, 0), (10, 10)] {
            column.set_block(IVec3::new(x, 64, z), id("packed_earth"));
            *column.wear_mut(x, z) = voxmar_engine::world::Wear { at: (z * 32 + x) as u16, steps: 3, stages: 2, day: 0 };
        }
        let days = rules.heal_days;
        rules.heal(&mut column, IVec3::ZERO, days as u32, &registry);
        // The pair heals one stage; the lone spot, twice as fast, both.
        assert_eq!(column.block(IVec3::new(0, 64, 0)), id("worn_grass_block"));
        assert_eq!(column.block(IVec3::new(10, 64, 10)), id("grass_block"));
        assert_eq!(column.wear_at(0, 0).map(|w| (w.steps, w.stages)), Some((0, 1)));
        assert!(column.wear_at(10, 10).is_none(), "fully healed spots are forgotten");
        // Never past what feet did: generated packed earth with no wear stays.
        column.set_block(IVec3::new(20, 64, 20), id("packed_earth"));
        rules.heal(&mut column, IVec3::ZERO, 1000, &registry);
        assert_eq!(column.block(IVec3::new(20, 64, 20)), id("packed_earth"));
        assert_eq!(column.block(IVec3::new(0, 64, 0)), id("grass_block"));
    }
}
