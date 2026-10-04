//! Stage 3: rivers and lakes, which run where water would.
//!
//! Water is routed in two tiers. Trunk rivers are laid out first, on a coarse grid of
//! 64-block cells over the land's broad shape: they gather the rain of whole regions,
//! and the terrain is then cut to them, with valleys, floodplains, gorges and tunnels, so
//! they always run downhill however far they go. Creeks and streams are routed afterwards
//! on a fine grid of 8-block cells over the land as the trunks left it, gathering water
//! from the hills until they reach a trunk, a lake or the sea. Both tiers use the same
//! rules (see `drainage`): steepest descent, pits breached or filled as lakes by how deep
//! they are and how humid the climate is, and flow summed from the rain on every cell
//! upstream, region by region, so a river is as big as its catchment says. Their rivers
//! are drawn into each column by `channel`.
//!
//! Everything here is a pure function of the seed and position: a region's rivers depend
//! only on its own ground and on the water the regions upstream send in, worked out on
//! demand and cached in caches bounded by bytes (about 400 MB in all, at most). No result depends on which column, region or
//! thread came first.
//!
//! Generated water is sealed: every water block lies on ground or water, and beside it
//! is ground, water, or the lower water a waterfall drops into, so nothing generated
//! needs to settle.

mod cache;
mod channel;
mod drainage;

use std::cell::RefCell;
use std::sync::Arc;

use voxmar_engine::glam::IVec2;

use self::cache::Cache;
use self::channel::{Carved, Channels, Span, Style, carve, top_block};
use self::drainage::{Budget, DIRS, Drainage, Ground, Tier, Tile, TunnelRule};
use crate::climate::{self, Climate, LAPSE_RATE, Regional};
use crate::dials::WorldDials;
use fastnoise_lite::FractalType;

use crate::geology::{GeoSample, Geology, noise, smoothstep};
use crate::{SEA_LEVEL, rng};

pub use self::channel::{Body, Cavity, Current};

/// Blocks along a side of a region, the unit in which rivers are routed and cached.
pub const REGION: i32 = 1024;
/// Cell sizes of the two tiers, in blocks.
const TRUNK_CELL: i32 = 64;
const STREAM_CELL: i32 = 8;
/// Cells along a side of a tile of stream-tier ground samples (256 blocks).
const STREAM_TILE: i32 = 32;
/// How far a region's trunks and streams reach beyond it, in blocks.
const TRUNK_MARGIN: i32 = 192;
const STREAM_MARGIN: i32 = 64;
/// Spacing of the climate samples rivers read rainfall from, in blocks.
const RAIN_CELL: i32 = 128;
const MB: usize = 1 << 20;
/// Area that counts as one unit of flow, in square blocks, at a runoff of 1.
const FLOW_AREA: f32 = 10_000.0;

/// The rivers and lakes of a world.
pub struct Rivers {
    trunks: Drainage,
    streams: Drainage,
    trunk_style: Style,
    stream_style: Style,
    coarse: Cache<IVec2, Arc<Tile>>,
    fine: Cache<IVec2, Arc<Tile>>,
    rain: Cache<IVec2, Arc<[Regional]>>,
    trunk_channels: Cache<IVec2, Arc<Channels>>,
    stream_channels: Cache<IVec2, Arc<Channels>>,
    /// Flow from which a creek is drawn, and from which a trunk river is.
    creek: f32,
    trunk: f32,
    /// Tells this world's entries apart in the per-thread cache of channels.
    id: u64,
}

/// The stages rivers read: the land, and the climate for rain.
#[derive(Clone, Copy)]
pub struct Stages<'a> {
    pub geology: &'a Geology,
    pub climate: &'a Climate,
}

struct TrunkGround<'a> {
    rivers: &'a Rivers,
    land: Stages<'a>,
}

struct StreamGround<'a> {
    rivers: &'a Rivers,
    land: Stages<'a>,
}

impl Ground for TrunkGround<'_> {
    fn tile(&self, key: IVec2) -> Arc<Tile> {
        self.rivers.coarse.get(key, || Arc::new(self.rivers.coarse_tile(self.land, key)))
    }

    fn humidity(&self, cell: IVec2) -> f32 {
        self.rivers.humidity(self.land, self.rivers.trunks.tier.centre(cell))
    }
}

impl Ground for StreamGround<'_> {
    fn tile(&self, key: IVec2) -> Arc<Tile> {
        self.rivers.fine.get(key, || Arc::new(self.rivers.fine_tile(self.land, key)))
    }

    fn humidity(&self, cell: IVec2) -> f32 {
        self.rivers.humidity(self.land, self.rivers.streams.tier.centre(cell))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Trunk,
    Stream,
}

/// Channels recently used by this thread, keyed by world, tier and region.
type Recent = Vec<(u64, Level, IVec2, Arc<Channels>)>;

thread_local! {
    /// The channels this thread used last, which spares the shared cache's lock for the
    /// many columns of a chunk.
    static RECENT: RefCell<Recent> = const { RefCell::new(Vec::new()) };
}

impl Rivers {
    pub fn new(seed: u64, dials: &WorldDials) -> Self {
        static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seed = rng::mix(seed ^ 0x51ee_4a7e);
        let trunks = Tier {
            cell: TRUNK_CELL,
            side: REGION / TRUNK_CELL,
            tile: REGION / TRUNK_CELL,
            lake_radius: 10,
            lake_cells: 200,
            breach: 6,
            min_depth: 4.0,
            min_area: 3,
            incision: 2.0,
            lakes: false,
            tunnel: Some(TunnelRule { depth: 14.0, chance: 0.35, length: 6 }),
            seed,
        };
        let streams = Tier {
            cell: STREAM_CELL,
            side: REGION / STREAM_CELL,
            tile: STREAM_TILE,
            lake_radius: 40,
            lake_cells: 4000,
            breach: 6,
            min_depth: 5.0,
            min_area: 48,
            incision: 1.0,
            lakes: true,
            tunnel: None,
            seed: rng::mix(seed),
        };
        // Wetter worlds have more creeks, starting from smaller catchments.
        let creek = 2.4 - 1.4 * dials.river_density;
        Self {
            trunks: Drainage::new(trunks, Budget { lakes: 16 * MB, basins: 32 * MB, flows: 8 * MB }),
            streams: Drainage::new(streams, Budget { lakes: 32 * MB, basins: 96 * MB, flows: 16 * MB }),
            trunk_style: Style {
                river_width: 5.0,
                floodplain: 1.0,
                wall: 1.6,
                valley: 48.0,
                jitter: 18.0,
                shore: 5,
                roof: 14.0,
                islands: noise(seed, 0x15, 1.0 / 36.0, FractalType::FBm, 2),
            },
            stream_style: Style {
                river_width: 5.0,
                floodplain: 0.3,
                wall: 1.1,
                valley: 20.0,
                jitter: 2.5,
                shore: 0,
                roof: f32::MAX,
                islands: noise(seed, 0x16, 1.0 / 36.0, FractalType::FBm, 2),
            },
            coarse: Cache::new(16 * MB),
            fine: Cache::new(64 * MB),
            rain: Cache::new(4 * MB),
            trunk_channels: Cache::new(32 * MB),
            stream_channels: Cache::new(96 * MB),
            creek,
            trunk: creek * 6.0,
            id: IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Width of a river carrying `flow`, in blocks: a trickle at the smallest creek, six
    /// blocks with twenty times its water, ten with fifty.
    fn width(&self, flow: f32) -> f32 {
        (flow / self.creek).powf(0.6).clamp(1.0, 28.0)
    }

    /// The regional climate at a position, from samples every `RAIN_CELL` blocks.
    fn rain_at(&self, land: Stages, x: i32, z: i32) -> Regional {
        let per = REGION / RAIN_CELL;
        let region = IVec2::new(x, z).div_euclid(IVec2::splat(REGION));
        let samples = self.rain.get(region, || {
            (0..(per + 1) * (per + 1))
                .map(|i| {
                    let p = region * REGION + IVec2::new(i % (per + 1), i / (per + 1)) * RAIN_CELL;
                    land.climate.regional(land.geology, p.x as f32, p.y as f32)
                })
                .collect()
        });
        let local = IVec2::new(x, z) - region * REGION;
        let cell = local / RAIN_CELL;
        let f = (local - cell * RAIN_CELL).as_vec2() / RAIN_CELL as f32;
        let at = |i: i32, j: i32| samples[((cell.y + j) * (per + 1) + cell.x + i) as usize];
        at(0, 0).lerp(at(1, 0), f.x).lerp(at(0, 1).lerp(at(1, 1), f.x), f.y)
    }

    /// Precipitation over evaporation at a position whose ground is `height` high.
    fn moisture(&self, land: Stages, x: i32, z: i32, height: f32) -> f32 {
        let regional = self.rain_at(land, x, z);
        let temperature = regional.sea_temperature - LAPSE_RATE * (height - SEA_LEVEL as f32).max(0.0);
        climate::moisture(regional.precipitation, temperature)
    }

    /// Water a block of ground sheds into rivers: a little even in dry country, rising
    /// with rainfall.
    fn runoff(&self, land: Stages, x: i32, z: i32, height: f32) -> f32 {
        0.1 + 0.7 * self.moisture(land, x, z, height).clamp(0.0, 1.6)
    }

    /// Whether lakes stay full: 0 in deserts, 1 from moist grassland up.
    fn humidity(&self, land: Stages, at: IVec2) -> f32 {
        let height = land.geology.land(at.x, at.y).height;
        smoothstep(0.2, 0.5, self.moisture(land, at.x, at.y, height))
    }

    /// The trunk tier's ground: for each 64-block cell, the lowest of five samples of the
    /// land over it, so trunks find the valleys between hills.
    fn coarse_tile(&self, land: Stages, key: IVec2) -> Tile {
        let tier = &self.trunks.tier;
        let n = (tier.tile * tier.tile) as usize;
        let (mut height, mut runoff) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for i in 0..tier.tile * tier.tile {
            let centre = tier.centre(key * tier.tile + IVec2::new(i % tier.tile, i / tier.tile));
            let h = [IVec2::ZERO, IVec2::new(-20, -20), IVec2::new(20, -20), IVec2::new(-20, 20), IVec2::new(20, 20)]
                .into_iter()
                .map(|o| land.geology.land(centre.x + o.x, centre.y + o.y).height)
                .fold(f32::MAX, f32::min);
            height.push(h);
            runoff.push(self.runoff(land, centre.x, centre.y, h) * (TRUNK_CELL * TRUNK_CELL) as f32 / FLOW_AREA);
        }
        Tile { height: height.into(), runoff: runoff.into(), absorbed: vec![drainage::DRY; n].into() }
    }

    /// The stream tier's ground: the land at each 8-block cell's centre as the trunks cut
    /// it, and which cells lie in a trunk's water.
    fn fine_tile(&self, land: Stages, key: IVec2) -> Tile {
        let tier = &self.streams.tier;
        let n = (tier.tile * tier.tile) as usize;
        let (mut height, mut runoff, mut absorbed) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
        let first = tier.centre(key * tier.tile);
        let trunks = self.near(land, Level::Trunk, TRUNK_MARGIN, first, first + (tier.tile - 1) * tier.cell);
        for i in 0..tier.tile * tier.tile {
            let centre = tier.centre(key * tier.tile + IVec2::new(i % tier.tile, i / tier.tile));
            let ground = Carved::land(land.geology.land(centre.x, centre.y).height);
            let carved = carve(&self.trunk_style, trunks.list(), centre.x, centre.y, ground);
            height.push(carved.ground);
            absorbed.push(match carved.water {
                Some(surface) if carved.cavity.is_none() => surface,
                _ => drainage::DRY,
            });
            runoff.push(self.runoff(land, centre.x, centre.y, carved.ground) * (STREAM_CELL * STREAM_CELL) as f32 / FLOW_AREA);
        }
        Tile { height: height.into(), runoff: runoff.into(), absorbed: absorbed.into() }
    }

    /// The rivers a tier draws from a region's cells, and the lakes over the region.
    fn build_channels(&self, level: Level, ground: &dyn Ground, region: IVec2) -> Channels {
        let (drainage, style, threshold, margin) = match level {
            Level::Trunk => (&self.trunks, &self.trunk_style, self.trunk, TRUNK_MARGIN),
            Level::Stream => (&self.streams, &self.stream_style, self.creek, STREAM_MARGIN),
        };
        let tier = &drainage.tier;
        let basin = drainage.basin(ground, region);
        let totals = drainage.totals(ground, &basin);
        let side = tier.side;
        let origin = region * side;
        let in_region = |c: IVec2| {
            let p = c - origin;
            ((0..side).contains(&p.x) && (0..side).contains(&p.y)).then(|| (p.y * side + p.x) as usize)
        };
        // Water through any cell that drains into the region or lies in it.
        let flow = |c: IVec2| match in_region(c) {
            Some(i) => totals[i],
            None => drainage.exit_total(ground, c),
        };
        let mut spans = Vec::new();
        for i in 0..side * side {
            let from = origin + IVec2::new(i % side, i / side);
            let q = totals[i as usize];
            let k = basin.next(from);
            if q < threshold || k >= 8 {
                continue;
            }
            let to = from + DIRS[k as usize];
            if basin.submerged(from) && basin.submerged(to) {
                continue;
            }
            // The main river upstream, for the curve's tangent.
            let mut before: Option<(f32, IVec2)> = None;
            for d in DIRS {
                let up = from - d;
                if basin.next(up) as usize == DIRS.iter().position(|&e| e == d).unwrap() {
                    let q = flow(up);
                    if before.is_none_or(|(best, _)| q > best) {
                        before = Some((q, up));
                    }
                }
            }
            let after = match basin.next(to) {
                k if k < 8 => Some(to + DIRS[k as usize]),
                _ => None,
            };
            spans.push(Span {
                from,
                to,
                before: before.map(|(_, c)| c),
                after,
                levels: (basin.level(from), basin.level(to)),
                width: self.width(q),
                tunnel: basin.tunnel(from),
            });
        }
        // Trunk basins are coarse; their lakes would leak past rims the coarse grid
        // cannot see, so only the stream tier draws lakes.
        let lakes = if tier.lakes { &basin.lakes[..] } else { &[] };
        Channels::new(style, tier.cell, tier.seed, region, REGION, margin, &spans, lakes)
    }

    fn channels(&self, land: Stages, level: Level, region: IVec2) -> Arc<Channels> {
        let found = RECENT.with(|recent| {
            let mut recent = recent.borrow_mut();
            let i = recent.iter().position(|(id, l, r, _)| *id == self.id && *l == level && *r == region)?;
            let channels = recent[i].3.clone();
            if i > 0 {
                let entry = recent.remove(i);
                recent.insert(0, entry);
            }
            Some(channels)
        });
        if let Some(channels) = found {
            return channels;
        }
        let channels = match level {
            Level::Trunk => self.trunk_channels.get(region, || {
                Arc::new(self.build_channels(level, &TrunkGround { rivers: self, land }, region))
            }),
            Level::Stream => self.stream_channels.get(region, || {
                Arc::new(self.build_channels(level, &StreamGround { rivers: self, land }, region))
            }),
        };
        RECENT.with(|recent| {
            let mut recent = recent.borrow_mut();
            recent.insert(0, (self.id, level, region, channels.clone()));
            recent.truncate(12);
        });
        channels
    }

    /// The channels of a tier that may reach any column from `min` to `max` (inclusive):
    /// those of every region within `margin` of them.
    fn near(&self, land: Stages, level: Level, margin: i32, min: IVec2, max: IVec2) -> Near {
        let first = (min - margin).div_euclid(IVec2::splat(REGION));
        let last = (max + margin).div_euclid(IVec2::splat(REGION));
        let mut near = Near::new();
        for z in first.y..=last.y {
            for x in first.x..=last.x {
                near.push(self.channels(land, level, IVec2::new(x, z)));
            }
        }
        near
    }

    /// Both tiers' channels near the columns from `min` to `max`.
    fn tiers(&self, land: Stages, min: IVec2, max: IVec2) -> (Near, Near) {
        (self.near(land, Level::Trunk, TRUNK_MARGIN, min, max), self.near(land, Level::Stream, STREAM_MARGIN, min, max))
    }

    /// A column with both tiers carved in, before sealing.
    fn unsealed(&self, land: Stages, (trunks, streams): &(Near, Near), x: i32, z: i32) -> Column {
        let ground = land.geology.land(x, z);
        let trunks = carve(&self.trunk_style, trunks.list(), x, z, Carved::land(ground.height));
        let carved = carve(&self.stream_style, streams.list(), x, z, trunks);
        let height = (carved.ground.floor() as i32).max(4);
        let water = match carved.water {
            Some(surface) => top_block(surface).max(if height < SEA_LEVEL { SEA_LEVEL } else { i32::MIN }),
            None if height < SEA_LEVEL => SEA_LEVEL,
            // Dry ground: the level of the water nearby, if it lies no higher than the
            // ground, which is what banks and shores are judged by.
            None => carved.near.map_or(SEA_LEVEL, |near| top_block(near).min(height)),
        };
        let submerged = water > height || carved.cavity.is_some_and(|c| water > c.floor);
        let body = match carved.body {
            Some(body) => body,
            None => Body::Sea,
        };
        Column {
            sample: GeoSample {
                height,
                mountain: ground.mountain,
                river: carved.wet,
                water,
                body: if height < SEA_LEVEL && carved.water.is_none() { Body::Sea } else { body },
                current: if submerged { carved.current } else { None },
                cavity: carved.cavity,
            },
            near: carved.near.is_some() || carved.wet > 0.0,
        }
    }

    /// The geology of one column, with its rivers and lakes.
    pub fn sample(&self, land: Stages, x: i32, z: i32) -> GeoSample {
        let tiers = self.tiers(land, IVec2::new(x - 1, z - 1), IVec2::new(x + 1, z + 1));
        let column = self.unsealed(land, &tiers, x, z);
        if !column.near || column.sample.water > column.sample.height {
            return column.sample;
        }
        let neighbours = [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|(dx, dz)| self.unsealed(land, &tiers, x + dx, z + dz).sample);
        seal(column.sample, &neighbours)
    }

    /// The geology of every column of a `size`×`size` square from `min`, with rivers and
    /// lakes; the same as sampling each alone, but sharing the work of sealing.
    pub fn area(&self, land: Stages, min: IVec2, size: i32) -> Vec<GeoSample> {
        let wide = size + 2;
        let tiers = self.tiers(land, min - 1, min + size);
        let columns: Vec<Column> = (0..wide * wide).map(|i| self.unsealed(land, &tiers, min.x - 1 + i % wide, min.y - 1 + i / wide)).collect();
        (0..size * size)
            .map(|i| {
                let (x, z) = (i % size + 1, i / size + 1);
                let column = &columns[(z * wide + x) as usize];
                if !column.near || column.sample.water > column.sample.height {
                    return column.sample;
                }
                let neighbours = [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|(dx, dz)| columns[((z + dz) * wide + x + dx) as usize].sample);
                seal(column.sample, &neighbours)
            })
            .collect()
    }
}

/// The channels of the regions around some columns: up to four, as a margin is far less
/// than a region.
struct Near {
    channels: [Option<Arc<Channels>>; 4],
    len: usize,
}

impl Near {
    fn new() -> Self {
        Self { channels: Default::default(), len: 0 }
    }

    fn push(&mut self, channels: Arc<Channels>) {
        assert!(self.len < 4, "columns near more than four regions");
        self.channels[self.len] = Some(channels);
        self.len += 1;
    }

    fn list(&self) -> &[Option<Arc<Channels>>] {
        &self.channels[..self.len]
    }
}

/// A column before sealing, and whether it lies near any river or lake.
struct Column {
    sample: GeoSample,
    near: bool,
}

/// Raises a dry column beside water to the water's surface, so no water stands beside air.
/// Water beside lower water is a waterfall and stays.
fn seal(mut sample: GeoSample, neighbours: &[GeoSample; 4]) -> GeoSample {
    for n in neighbours {
        if n.water > n.height && n.water > sample.height && n.cavity.is_none() {
            sample.height = n.water;
        }
    }
    sample
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dials::WorldDials;

    /// `cargo test --release -p voxmar_worldgen river_stats -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn river_stats() {
        let seed = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1u64);
        let stream = rng::stream_seed(seed, crate::Overworld::ID, crate::Overworld::VERSION);
        let dials = WorldDials::sample(stream);
        let geology = Geology::new(stream, &dials);
        let climate = Climate::new(stream, &dials);
        let rivers = Rivers::new(stream, &dials);
        let land = Stages { geology: &geology, climate: &climate };
        let x = std::env::var("X").ok().and_then(|s| s.parse().ok()).unwrap_or(2544);
        let z = std::env::var("Z").ok().and_then(|s| s.parse().ok()).unwrap_or(384);
        let region = IVec2::new(x, z).div_euclid(IVec2::splat(REGION));
        println!("creek {} trunk {} dials {dials:?}", rivers.creek, rivers.trunk);
        for (name, drainage, ground) in [
            ("trunk", &rivers.trunks, &TrunkGround { rivers: &rivers, land } as &dyn Ground),
            ("stream", &rivers.streams, &StreamGround { rivers: &rivers, land } as &dyn Ground),
        ] {
            let basin = drainage.basin(ground, region);
            let totals = drainage.totals(ground, &basin);
            let mut sorted: Vec<f32> = totals.to_vec();
            sorted.sort_by(f32::total_cmp);
            let pct = |p: f32| sorted[((sorted.len() - 1) as f32 * p) as usize];
            let mut kinds = std::collections::BTreeMap::new();
            for lake in &basin.lakes {
                *kinds.entry(format!("{:?} wet {}", lake.kind, lake.wet())).or_insert(0) += 1;
            }
            let ends = (0..drainage.tier.side * drainage.tier.side)
                .filter(|&i| basin.next(region * drainage.tier.side + IVec2::new(i % drainage.tier.side, i / drainage.tier.side)) == drainage::END)
                .count();
            println!("{name}: flow p50 {:.2} p90 {:.2} p99 {:.2} max {:.2}; ends {ends}; lakes {kinds:?}", pct(0.5), pct(0.9), pct(0.99), pct(1.0));
        }
    }
}
