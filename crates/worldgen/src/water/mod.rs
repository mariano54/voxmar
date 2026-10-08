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
//! Generated water is settled: it is a fixed point of the fluid rule, so loading it costs
//! nothing (see `settle`, and the curtains of falls the terrain stamps).

pub(crate) mod cache;
mod channel;
mod drainage;
mod sights;

use std::cell::RefCell;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use voxmar_engine::glam::{IVec2, IVec3};

use self::cache::Cache;
use self::channel::{Carved, Channels, Span, Style, carve, top_block};
use self::drainage::{Budget, DIRS, Drainage, Ground, Reader, Tier, Tile, TunnelRule};
use crate::climate::{self, Climate, LAPSE_RATE, Regional};
use crate::dials::WorldDials;
use fastnoise_lite::FractalType;

use crate::density::Crag;
use crate::geology::{Corners, GeoSample, Geology, Landform, noise, smoothstep};
use crate::stage::{self, Stage};
use crate::{SEA_LEVEL, rng};

pub use self::channel::{Body, Cavity, Current};
pub use self::sights::WaterSight;

/// How often generation had to break a circle of water across region borders, in tests.
#[cfg(test)]
pub(crate) fn drainage_loops_ended() -> usize {
    drainage::LOOPS_ENDED.load(std::sync::atomic::Ordering::Relaxed)
}

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

/// Maximum centre samples sharing one bounded planning batch.
pub const SAMPLE_BATCH: usize = 32;

#[cfg(test)]
thread_local! {
    static QUERY_WORK: std::cell::Cell<Option<[usize; 8]>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn profile_queries(enabled: bool) {
    QUERY_WORK.with(|c| c.set(enabled.then_some([0; 8])));
}

#[cfg(test)]
pub(crate) fn query_work() -> Option<[usize; 8]> { QUERY_WORK.with(|c| c.get()) }

#[cfg(test)]
fn record_query(work: [usize; 8]) {
    QUERY_WORK.with(|c| {
        if let Some(previous) = c.get() { c.set(Some(std::array::from_fn(|i| previous[i] + work[i]))); }
    });
}

pub(crate) fn freshwater(g: &GeoSample) -> bool {
    (g.submerged() || g.river > 0.5) && g.body != Body::Sea
}
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

    fn has(&self, key: IVec2) -> bool {
        self.rivers.coarse.has(key)
    }

    fn humidity(&self, cell: IVec2) -> f32 {
        self.rivers.humidity(self.land, self.rivers.trunks.tier.centre(cell))
    }
}

impl Ground for StreamGround<'_> {
    fn tile(&self, key: IVec2) -> Arc<Tile> {
        self.rivers.fine.get(key, || Arc::new(self.rivers.fine_tile(self.land, key)))
    }

    fn has(&self, key: IVec2) -> bool {
        self.rivers.fine.has(key)
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
            // Trunks lie well down in the valleys they cut.
            incision: 3.0,
            lakes: false,
            springs: 0.0,
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
            // Creeks and streams run a block below the ground beside them.
            incision: 1.0,
            lakes: true,
            // Rare: a few in a volcanic world's kilometre, none in a quiet one.
            springs: 0.02 * smoothstep(0.35, 0.9, dials.volcanism),
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
                shore_run: 4.0,
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
                shore_run: 2.5,
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

    /// Width of a river carrying `flow`, in blocks: two and a half at the smallest creek,
    /// seven and a half with twenty times its water, eleven and a half with fifty.
    fn width(&self, flow: f32) -> f32 {
        (1.5 + (flow / self.creek).powf(0.6)).clamp(2.5, 28.0)
    }

    /// The regional climate over the region holding (x, z), sampled every `RAIN_CELL`
    /// blocks, for [`Rain::at`].
    fn rain(&self, land: Stages, x: i32, z: i32) -> Rain {
        let per = REGION / RAIN_CELL;
        let region = IVec2::new(x, z).div_euclid(IVec2::splat(REGION));
        let samples = self.rain.get(region, || {
            let points: Vec<(f32, f32)> = (0..(per + 1) * (per + 1))
                .map(|i| {
                    let p = region * REGION + IVec2::new(i % (per + 1), i / (per + 1)) * RAIN_CELL;
                    (p.x as f32, p.y as f32)
                })
                .collect();
            land.climate.regionals(land.geology, &points).into()
        });
        Rain { region, samples }
    }

    /// Precipitation over evaporation at a position whose ground is `height` high.
    fn moisture(&self, land: Stages, x: i32, z: i32, height: f32) -> f32 {
        self.rain(land, x, z).moisture(x, z, height)
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
        // A tile lies in one region, whose rain it reads.
        let rain = self.rain(land, key.x * tier.tile * tier.cell, key.y * tier.tile * tier.cell);
        const AROUND: [IVec2; 5] = [IVec2::ZERO, IVec2::new(-20, -20), IVec2::new(20, -20), IVec2::new(-20, 20), IVec2::new(20, 20)];
        let centres: Vec<IVec2> = (0..tier.tile * tier.tile).map(|i| tier.centre(key * tier.tile + IVec2::new(i % tier.tile, i / tier.tile))).collect();
        let points: Vec<(i32, i32)> = centres.iter().flat_map(|c| AROUND.map(|o| (c.x + o.x, c.y + o.y))).collect();
        let samples = land.geology.lands(&points);
        for (centre, samples) in centres.into_iter().zip(samples.chunks(AROUND.len())) {
            let h = samples.iter().map(|s| s.height).fold(f32::MAX, f32::min);
            height.push(h);
            runoff.push(rain.runoff(centre.x, centre.y, h) * (TRUNK_CELL * TRUNK_CELL) as f32 / FLOW_AREA);
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
        let rain = self.rain(land, first.x, first.y);
        let centres: Vec<IVec2> = (0..tier.tile * tier.tile).map(|i| tier.centre(key * tier.tile + IVec2::new(i % tier.tile, i / tier.tile))).collect();
        let grounds = land.geology.lands(&centres.iter().map(|c| (c.x, c.y)).collect::<Vec<_>>());
        for (centre, ground) in centres.into_iter().zip(grounds) {
            let ground = Carved::land(ground.height);
            let carved = carve(&self.trunk_style, trunks.list(), centre.x, centre.y, ground);
            height.push(carved.ground);
            absorbed.push(match carved.water {
                Some(surface) if carved.cavity.is_none() => surface,
                _ => drainage::DRY,
            });
            runoff.push(rain.runoff(centre.x, centre.y, carved.ground) * (STREAM_CELL * STREAM_CELL) as f32 / FLOW_AREA);
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
        // How far the ground at a cell's centre stands over its river, beyond the tier's
        // incision: next to nothing, except where a breach cuts through a rim.
        let mut reader = Reader::new(tier, ground);
        let mut cut = |cell: IVec2| reader.at(cell).0 + 1.0 - tier.incision - basin.level(cell) as f32 / 8.0;
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
                cut: (cut(from), cut(to)),
                width: self.width(q),
                tunnel: basin.tunnel(from),
            });
        }
        // Trunk basins are coarse; their lakes would leak past rims the coarse grid
        // cannot see, so only the stream tier draws lakes.
        let lakes = if tier.lakes { &basin.lakes[..] } else { &[] };
        Channels::new(style, tier.cell, tier.seed, region, REGION, margin, &spans, lakes)
    }

    /// Places within `radius` blocks of `from` where a river runs deep in a slot gorge (at
    /// least `GORGE_FOUND` of one), nearest first, as far as `enough` of them: each the
    /// column and the height of its top water block. Searches region by region outwards,
    /// building their rivers as it goes, so it is quick when some lie near.
    pub(crate) fn gorges_near(&self, land: Stages, from: IVec2, radius: i32, enough: usize) -> Vec<IVec3> {
        const GORGE_FOUND: f32 = 0.75;
        let (lo, hi) = ((from - radius).div_euclid(IVec2::splat(REGION)), (from + radius).div_euclid(IVec2::splat(REGION)));
        // Regions by how near they come to `from`.
        let mut regions: Vec<(f32, IVec2)> = (lo.y..=hi.y)
            .flat_map(|rz| (lo.x..=hi.x).map(move |rx| IVec2::new(rx, rz)))
            .map(|region| {
                let nearest = from.clamp(region * REGION, region * REGION + REGION - 1);
                ((nearest - from).as_vec2().length(), region)
            })
            .filter(|&(d, _)| d <= radius as f32)
            .collect();
        regions.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut found: Vec<(f32, IVec3)> = Vec::new();
        for (near, region) in regions {
            if found.len() >= enough && found[enough - 1].0 <= near {
                break;
            }
            for level in [Level::Trunk, Level::Stream] {
                for (at, water, gorge) in self.channels(land, level, region).gorges() {
                    let d = (at - from.as_vec2()).length();
                    if gorge >= GORGE_FOUND && d <= radius as f32 {
                        found.push((d, IVec3::new(at.x.floor() as i32, water, at.y.floor() as i32)));
                    }
                }
            }
            found.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        found.into_iter().map(|(_, at)| at).collect()
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
                let _stage = stage::enter(Stage::RiverRegions);
                Arc::new(self.build_channels(level, &TrunkGround { rivers: self, land }, region))
            }),
            Level::Stream => self.stream_channels.get(region, || {
                let _stage = stage::enter(Stage::RiverRegions);
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

    /// How many regions' trunk and stream channels are made, for tests of how much a
    /// piece of generation needs.
    #[cfg(test)]
    pub(crate) fn made(&self) -> (usize, usize) {
        (self.trunk_channels.len(), self.stream_channels.len())
    }

    /// Whether a tier's channels of a region are made or being made.
    fn known(&self, level: Level, region: IVec2) -> bool {
        RECENT.with(|recent| recent.borrow().iter().any(|(id, l, r, _)| *id == self.id && *l == level && *r == region))
            || match level {
                Level::Trunk => self.trunk_channels.has(region),
                Level::Stream => self.stream_channels.has(region),
            }
    }

    /// The regions within `margin` of the columns from `min` to `max` (inclusive), row by
    /// row.
    fn regions(margin: i32, min: IVec2, max: IVec2) -> impl Iterator<Item = IVec2> {
        let first = (min - margin).div_euclid(IVec2::splat(REGION));
        let last = (max + margin).div_euclid(IVec2::splat(REGION));
        (first.y..=last.y).flat_map(move |z| (first.x..=last.x).map(move |x| IVec2::new(x, z)))
    }

    /// The channels of a tier that may reach any column from `min` to `max` (inclusive):
    /// those of every region within `margin` of them.
    fn near(&self, land: Stages, level: Level, margin: i32, min: IVec2, max: IVec2) -> Near {
        let mut near = Near::new();
        for region in Self::regions(margin, min, max) {
            near.push(self.channels(land, level, region));
        }
        near
    }

    /// Both tiers' channels near the columns from `min` to `max`. Those not made yet are
    /// made first, side by side on every core (see `crate::parallel`), rather than one
    /// after another with only their tiles shared out: a new world's first columns need
    /// several of each.
    fn tiers(&self, land: Stages, min: IVec2, max: IVec2) -> (Near, Near) {
        let missing: Vec<(Level, IVec2)> = [(Level::Trunk, TRUNK_MARGIN), (Level::Stream, STREAM_MARGIN)]
            .into_iter()
            .flat_map(|(level, margin)| Self::regions(margin, min, max).map(move |region| (level, region)))
            .filter(|&(level, region)| !self.known(level, region))
            .collect();
        if missing.len() > 1 {
            crate::parallel::for_each(&missing, |&(level, region)| {
                self.channels(land, level, region);
            });
        }
        (self.near(land, Level::Trunk, TRUNK_MARGIN, min, max), self.near(land, Level::Stream, STREAM_MARGIN, min, max))
    }

    /// A column with both tiers carved in, before sealing, on its `ground`.
    fn unsealed(&self, (trunks, streams): &(Near, Near), x: i32, z: i32, ground: Landform) -> Column {
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
        // Eighths of the top block the surface fills; the sea, and rivers meeting it, fill it.
        let top = match carved.water {
            Some(surface) if top_block(surface) == water => (surface - 8 * water) as u8,
            _ => 8,
        };
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
                top,
                body: if height < SEA_LEVEL && carved.water.is_none() { Body::Sea } else { body },
                current: if submerged { carved.current } else { None },
                cavity: carved.cavity,
                // Rock breaks up into crags only away from water, which the hollows would
                // drain, and over the sea and the water nearby.
                crag: Crag {
                    surface: carved.ground,
                    amplitude: if carved.near.is_some() || carved.wet > 0.0 || carved.cavity.is_some() { 0.0 } else { ground.crag },
                    floor: water.max(SEA_LEVEL) + 1,
                },
                form: ground.form,
                trail: None,
            },
            near: carved.near.is_some() || carved.wet > 0.0,
        }
    }

    /// The geology of one column, with its rivers and lakes; with `crags`, a crag's height
    /// is the top of its density rather than its preliminary surface.
    pub fn sample(&self, land: Stages, x: i32, z: i32, crags: bool) -> GeoSample {
        let tiers = self.tiers(land, IVec2::new(x - 1, z - 1), IVec2::new(x + 1, z + 1));
        let mut corners = Corners::default();
        let ground = land.geology.lands_with(&[(x, z)], &mut corners)[0];
        let mut sample = [self.sealed(land, &tiers, x, z, ground, &mut corners)];
        if crags {
            crag_tops(land.geology, &mut sample, |_| (x, z));
        }
        sample[0]
    }

    /// A planning sample, sharing relief corners and exactly the same channel regions
    /// as standalone samples. The scratch storage belongs to one planning call.
    pub fn sample_cached(&self, land: Stages, x: i32, z: i32, scratch: &mut Samples) -> GeoSample {
        scratch.prepare(self.id, 1);
        let (min, max) = (IVec2::new(x - 1, z - 1), IVec2::new(x + 1, z + 1));
        let key = Samples::key(min, max);
        let tiers = scratch.tiers.entry(key).or_insert_with(|| self.tiers(land, min, max));
        let p = IVec2::new(x, z);
        let ground = (!scratch.columns.contains_key(&(p, key))).then(|| land.geology.lands_with(&[(x, z)], &mut scratch.corners)[0]);
        self.sealed_cached(land, tiers, key, p, ground, &mut scratch.corners, &mut scratch.columns)
    }

    /// Exact standalone samples in a bounded batch: only the geology of these centres
    /// is grouped. Every centre keeps its own p +/- 1 channel bounds and sealed neighbours.
    pub fn samples_cached(&self, land: Stages, points: &[(i32, i32)], scratch: &mut Samples) -> Vec<GeoSample> {
        assert!(points.len() <= SAMPLE_BATCH);
        scratch.prepare(self.id, points.len());
        if points.is_empty() { return Vec::new(); }
        let keys: Vec<_> = points.iter().map(|&(x, z)| Samples::key(IVec2::new(x - 1, z - 1), IVec2::new(x + 1, z + 1))).collect();
        let needed: Vec<_> = points.iter().zip(&keys).map(|(&(x, z), &key)| !scratch.columns.contains_key(&(IVec2::new(x, z), key))).collect();
        let missing: Vec<_> = points.iter().zip(&needed).filter_map(|(&p, &need)| need.then_some(p)).collect();
        let mut grounds = land.geology.lands_with(&missing, &mut scratch.corners).into_iter();
        points.iter().enumerate().map(|(i, &(x, z))| {
            // Clear only channel groups here: the already computed centre landforms
            // remain valid, and the reserved corner headroom covers all neighbours.
            if scratch.tiers.len() >= 16 { scratch.tiers.clear(); }
            let (min, max) = (IVec2::new(x - 1, z - 1), IVec2::new(x + 1, z + 1));
            let tiers = scratch.tiers.entry(keys[i]).or_insert_with(|| self.tiers(land, min, max));
            // Keep the initial miss mask: an earlier neighbour may have populated
            // this column since its landform was batched, but that work still counts.
            let ground = needed[i].then(|| grounds.next().expect("batched missing landform"));
            self.sealed_cached(land, tiers, keys[i], IVec2::new(x, z), ground, &mut scratch.corners, &mut scratch.columns)
        }).collect()
    }

    /// Remember the exact sealed values from this river land's coarse site grid.
    /// Channel bounds of a grid include each centre's halo, so the full GeoSample is
    /// identical to standalone sampling. No raw column is given different tier keys.
    pub(crate) fn remember_freshwater_grid(&self, origin: IVec2, step: i32, size: i32, samples: &[GeoSample], cache: &mut Freshwater) {
        assert!(size > 0 && step > 0 && samples.len() == (size as usize) * (size as usize));
        if cache.id != self.id { *cache = Freshwater { id: self.id, ..Freshwater::default() }; }
        if cache.seen.len() + samples.len() > 16_384 { cache.seen.clear(); }
        for (i, &sample) in samples.iter().take(16_384).enumerate() {
            let p = origin + IVec2::new(i as i32 % size, i as i32 / size) * step;
            cache.seen.insert(p, sample);
        }
    }

    /// The same sampled freshwater test as a full close-look grid. Consume its circle
    /// row by row and stop after a matching batch; no outside-circle centre is needed,
    /// but sealing still reads all four neighbours, including those outside the circle.
    /// Callers supply radius >= 0 and step >= 1, as settlement loading validates.
    pub fn freshwater_near(&self, land: Stages, centre: IVec2, radius: i32, step: i32, cache: &mut Freshwater) -> bool {
        let first = centre - radius;
        let size = radius * 2 / step + 1;
        let changed_world = cache.id != self.id;
        if changed_world { *cache = Freshwater { id: self.id, ..Freshwater::default() }; }
        let mut points = Vec::with_capacity(SAMPLE_BATCH);
        #[cfg(test)]
        let profiling = query_work().is_some();
        #[cfg(test)]
        let mut work = [1, (size * size) as usize, 0, 0, 0, 0, 0, usize::from(changed_world)];
        for i in 0..size * size {
            let p = first + IVec2::new(i % size, i / size) * step;
            if p.as_vec2().distance(centre.as_vec2()) <= radius as f32 {
                if let Some(g) = cache.seen.get(&p) {
                    #[cfg(test)]
                    if profiling { work[6] += 1; }
                    // A known exact witness is enough for this existential query,
                    // even if earlier missing centres are still waiting for a batch.
                    if freshwater(g) {
                        #[cfg(test)]
                        if profiling { work[2] = i as usize + 1; record_query(work); }
                        return true;
                    }
                } else {
                    points.push((p.x, p.y));
                }
            }
            if points.len() == SAMPLE_BATCH {
                let (found, cleared) = self.freshwater_batch(land, &points, cache);
                #[cfg(test)]
                if profiling {
                    work[2] = i as usize + 1;
                    work[3] += points.len();
                    work[4] += 1;
                    work[7] += usize::from(cleared);
                    if let Some(k) = found { work[5] += points.len() - k - 1; }
                }
                let _ = cleared;
                if found.is_some() {
                    #[cfg(test)]
                    if profiling { record_query(work); }
                    return true;
                }
                points.clear();
            }
        }
        let (found, cleared) = if points.is_empty() { (None, false) } else { self.freshwater_batch(land, &points, cache) };
        #[cfg(test)]
        if profiling {
            work[2] = (size * size) as usize;
            work[3] += points.len();
            work[4] += usize::from(!points.is_empty());
            work[7] += usize::from(cleared);
            if let Some(k) = found { work[5] += points.len() - k - 1; }
            record_query(work);
        }
        let _ = cleared;
        found.is_some()
    }

    fn freshwater_batch(&self, land: Stages, points: &[(i32, i32)], cache: &mut Freshwater) -> (Option<usize>, bool) {
        let cleared = cache.seen.len() + points.len() > 16_384;
        if cleared { cache.seen.clear(); }
        let samples = self.samples_cached(land, points, &mut cache.scratch);
        let found = samples.iter().position(freshwater);
        cache.seen.extend(points.iter().zip(samples).map(|(&(x, z), g)| (IVec2::new(x, z), g)));
        (found, cleared)
    }

    /// One column with both tiers carved in and sealed: the same whatever the columns
    /// `tiers` were found for, as long as they include it and its neighbours. (A region
    /// whose index does not reach a column adds nothing to it, and `near` keeps regions
    /// in the same order however many it finds.)
    /// The land's lattice points already worked out are shared through `corners`, as
    /// neighbouring columns read the same ones.
    fn sealed(&self, land: Stages, tiers: &(Near, Near), x: i32, z: i32, ground: Landform, corners: &mut Corners) -> GeoSample {
        let column = self.unsealed(tiers, x, z, ground);
        if !column.near {
            return column.sample;
        }
        let around = [(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)];
        let grounds = land.geology.lands_with(&around, corners);
        let neighbours = std::array::from_fn(|k| self.unsealed(tiers, around[k].0, around[k].1, grounds[k]).sample);
        settle(column.sample, &neighbours)
    }

    /// Reuse only unsealed columns computed under these exact channel bounds. A
    /// neighbour keeps the requesting centre's key, not its own standalone halo key.
    #[allow(clippy::too_many_arguments)]
    fn sealed_cached(&self, land: Stages, tiers: &(Near, Near), key: [IVec2; 4], p: IVec2, ground: Option<Landform>, corners: &mut Corners, columns: &mut FxHashMap<(IVec2, [IVec2; 4]), Column>) -> GeoSample {
        let column = *columns.entry((p, key)).or_insert_with(|| self.unsealed(tiers, p.x, p.y, ground.expect("uncached centre landform")));
        if !column.near { return column.sample; }
        let around = [p + IVec2::X, p - IVec2::X, p + IVec2::Y, p - IVec2::Y];
        let missing: Vec<_> = around.iter().filter(|&&at| !columns.contains_key(&(at, key))).map(|at| (at.x, at.y)).collect();
        if !missing.is_empty() {
            let grounds = land.geology.lands_with(&missing, corners);
            for ((x, z), ground) in missing.into_iter().zip(grounds) {
                columns.insert((IVec2::new(x, z), key), self.unsealed(tiers, x, z, ground));
            }
        }
        let neighbours = around.map(|at| columns[&(at, key)].sample);
        settle(column.sample, &neighbours)
    }

    /// The geology of the `size`×`size` columns `step` blocks apart from `origin`, row by
    /// row: the same as sampling each without crags, finding the rivers near them once.
    pub fn grid(&self, land: Stages, origin: IVec2, step: i32, size: i32) -> Vec<GeoSample> {
        let (min, max) = (origin - 1, origin + (size - 1) * step + 1);
        let regions = ((max + TRUNK_MARGIN).div_euclid(IVec2::splat(REGION)) - (min - TRUNK_MARGIN).div_euclid(IVec2::splat(REGION)) + 1).element_product();
        let at = |i: i32| origin + IVec2::new(i % size, i / size) * step;
        let mut corners = Corners::default();
        let grounds = land.geology.lands_with(&(0..size * size).map(|i| (at(i).x, at(i).y)).collect::<Vec<_>>(), &mut corners);
        if regions > 4 {
            return (0..size * size)
                .map(|i| {
                    let tiers = self.tiers(land, at(i) - 1, at(i) + 1);
                    self.sealed(land, &tiers, at(i).x, at(i).y, grounds[i as usize], &mut corners)
                })
                .collect();
        }
        let tiers = self.tiers(land, min, max);
        (0..size * size).map(|i| self.sealed(land, &tiers, at(i).x, at(i).y, grounds[i as usize], &mut corners)).collect()
    }

    /// The geology of every column of a `size`×`size` square from `min`, with rivers and
    /// lakes; the same as sampling each alone, but sharing the work of sealing.
    pub fn area(&self, land: Stages, min: IVec2, size: i32) -> Vec<GeoSample> {
        let wide = size + 2;
        let tiers = self.tiers(land, min - 1, min + size);
        let at = |i: i32| (min.x - 1 + i % wide, min.y - 1 + i / wide);
        let grounds = land.geology.lands(&(0..wide * wide).map(at).collect::<Vec<_>>());
        let columns: Vec<Column> = (0..wide * wide).map(|i| self.unsealed(&tiers, at(i).0, at(i).1, grounds[i as usize])).collect();
        let mut samples: Vec<GeoSample> = (0..size * size)
            .map(|i| {
                let (x, z) = (i % size + 1, i / size + 1);
                let column = &columns[(z * wide + x) as usize];
                if !column.near {
                    return column.sample;
                }
                let neighbours = [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|(dx, dz)| columns[((z + dz) * wide + x + dx) as usize].sample);
                settle(column.sample, &neighbours)
            })
            .collect();
        crag_tops(land.geology, &mut samples, |i| (min.x + i as i32 % size, min.y + i as i32 / size));
        samples
    }
}

/// Temporary terrain work reused within a planner, never retained by a world cache.
/// Switching worlds resets it so corners and channels cannot leak between seeds.
#[derive(Default)]
pub struct Samples {
    id: u64,
    corners: Corners,
    tiers: FxHashMap<[IVec2; 4], (Near, Near)>,
    columns: FxHashMap<(IVec2, [IVec2; 4]), Column>,
}

/// Exact water-bearing terrain shared by a site's overlapping close looks. Only
/// original land is stored, bounded to 16,384 centres; a different world resets it.
#[derive(Default)]
pub struct Freshwater {
    id: u64,
    seen: FxHashMap<IVec2, GeoSample>,
    scratch: Samples,
}

impl Samples {
    fn prepare(&mut self, id: u64, count: usize) {
        // Each centre and its four cardinal neighbours add at most twenty corners.
        // Reserve the whole batch before any centre or neighbour work is performed.
        if self.id != id {
            *self = Self { id, ..Self::default() };
        }
        if self.corners.len() + 20 * count > 4096 { self.corners = Corners::default(); }
        // A batch inserts at most its centres and four neighbours per centre.
        if self.columns.len() + 5 * count > 16_384 { self.columns.clear(); }
        if self.tiers.len() >= 16 { self.tiers.clear(); }
    }

    fn key(min: IVec2, max: IVec2) -> [IVec2; 4] {
        let region = |at: IVec2| at.div_euclid(IVec2::splat(REGION));
        [region(min - TRUNK_MARGIN), region(max + TRUNK_MARGIN), region(min - STREAM_MARGIN), region(max + STREAM_MARGIN)]
    }
}

/// The regional climate over one region, sampled every `RAIN_CELL` blocks.
struct Rain {
    region: IVec2,
    samples: Arc<[Regional]>,
}

impl Rain {
    /// The regional climate at a position in the region, interpolated.
    fn at(&self, x: i32, z: i32) -> Regional {
        let per = REGION / RAIN_CELL;
        let local = IVec2::new(x, z) - self.region * REGION;
        debug_assert!(local.cmpge(IVec2::ZERO).all() && local.cmplt(IVec2::splat(REGION)).all(), "({x}, {z}) outside region {}", self.region);
        let cell = local / RAIN_CELL;
        let f = (local - cell * RAIN_CELL).as_vec2() / RAIN_CELL as f32;
        let at = |i: i32, j: i32| self.samples[((cell.y + j) * (per + 1) + cell.x + i) as usize];
        at(0, 0).lerp(at(1, 0), f.x).lerp(at(0, 1).lerp(at(1, 1), f.x), f.y)
    }

    /// Precipitation over evaporation at a position whose ground is `height` high.
    fn moisture(&self, x: i32, z: i32, height: f32) -> f32 {
        let regional = self.at(x, z);
        let temperature = regional.sea_temperature - LAPSE_RATE * (height - SEA_LEVEL as f32).max(0.0);
        climate::moisture(regional.precipitation, temperature)
    }

    /// Water a block of ground sheds into rivers: a little even in dry country, rising
    /// with rainfall.
    fn runoff(&self, x: i32, z: i32, height: f32) -> f32 {
        0.1 + 0.7 * self.moisture(x, z, height).clamp(0.0, 1.6)
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
#[derive(Clone, Copy)]
struct Column {
    sample: GeoSample,
    near: bool,
}

/// Raises or lowers each sample with a crag to the top of its density (`crate::density`),
/// all at once, as neighbours share the noise's lattice points. Sealing never reads a crag's
/// height (crags keep away from water), so this comes after it.
fn crag_tops(geology: &Geology, samples: &mut [GeoSample], at: impl Fn(usize) -> (i32, i32)) {
    let crags: Vec<usize> = (0..samples.len()).filter(|&i| samples[i].crag.amplitude > 0.0).collect();
    if crags.is_empty() {
        return;
    }
    let columns: Vec<(i32, i32, Crag)> = crags.iter().map(|&i| (at(i).0, at(i).1, samples[i].crag)).collect();
    for (&i, top) in crags.iter().zip(geology.density().tops(&columns)) {
        samples[i].height = top;
    }
}

/// Shapes a column so its water is a fixed point of the fluid rule (see "Generated water"
/// in `voxmar_engine::fluid`), given its four neighbours before this:
/// - a dry column beside water rises to the water's level, so water never stands beside
///   air, except at a fall, and takes that level as the water it borders;
/// - water whose top block stands above a neighbour's fills that block by one eighth
///   only, which pours nothing over the step;
/// - water two or more blocks above a neighbour's runs over a sill of ground just above
///   that neighbour's water, so the curtain a fall pours (which the terrain stamps) has no
///   upper water beside its foot, where water lying on more water would spread;
/// - over a drop of three or more, the water at the lip is at least a block deep under
///   its top eighth, so the fall has a full block of water beside it to pour from.
fn settle(mut sample: GeoSample, neighbours: &[GeoSample; 4]) -> GeoSample {
    let Some((_, top)) = sample.wet() else {
        // The water beside it is the water it is the bank of, which trails cut down to.
        for n in neighbours {
            if n.water > n.height && n.cavity.is_none() {
                sample.height = sample.height.max(n.water);
                sample.water = sample.water.max(n.water);
            }
        }
        return sample;
    };
    let (mut sill, mut lip) = (i32::MIN, false);
    for n in neighbours {
        let Some((_, below)) = n.wet() else { continue };
        if below < top {
            sample.top = 1;
        }
        if below <= top - 2 {
            sill = sill.max(below + 1);
        }
        lip |= below <= top - 3;
    }
    if sample.cavity.is_none() {
        // At the lip of a fall, a full block of water lies under the eighth, so the fall
        // has water to pour.
        if lip {
            sample.height = sample.height.min(top - 2);
        }
        sample.height = sample.height.max(sill);
    }
    sample
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dials::WorldDials;

    #[test]
    fn planning_samples_match_standalone_across_water_boundaries_and_worlds() {
        let registry = Arc::new(voxmar_engine::block::BlockRegistry::from_ron("base", include_str!("../../../../assets/content/base/blocks.ron")).unwrap());
        let mut points = Vec::new();
        // The actual seed-7 river settlement's search area includes both narrow water
        // and dry banks, not just an empty plane where every implementation agrees.
        for z in 0..25 {
            for x in 0..25 {
                points.push((-1976 + 16 * x, 552 + 16 * z));
            }
        }
        // Dense, overlapping centre/neighbour work near the same actual river.
        for z in 0..9 {
            for x in 0..35 { points.push((-1793 + x, 748 + z)); }
        }
        // Each side of the exact tier-group transitions, including negative regions
        // and the one-block halo used to seal water against its four neighbours.
        for margin in [TRUNK_MARGIN, STREAM_MARGIN] {
            for boundary in [-REGION, 0, REGION] {
                for delta in [-1, 0, 1] {
                    for sign in [-1, 1] {
                        let edge = boundary + sign * (margin + 1) + delta;
                        points.extend([(edge, boundary + 17), (boundary - 19, edge)]);
                    }
                }
            }
        }
        let mut scratch = Samples::default();
        let mut wet = 0;
        let mut dry = 0;
        for seed in [7, 42, 7] {
            let world = crate::Overworld::new(seed, registry.clone());
            for reverse in [false, true] {
                for i in 0..points.len() {
                    let (x, z) = points[if reverse { points.len() - i - 1 } else { i }];
                    let expected = world.rivers.sample(world.stages(), x, z, false);
                    let actual = world.rivers.sample_cached(world.stages(), x, z, &mut scratch);
                    assert_eq!(actual, expected, "seed {seed} at {x},{z}, reverse {reverse}");
                    assert!(scratch.corners.len() <= 4096);
                    assert!(scratch.tiers.len() <= 16);
                    assert!(scratch.columns.len() <= 16_384);
                    if expected.submerged() && expected.body != Body::Sea { wet += 1; } else if !expected.submerged() { dry += 1; }
                }
                let ordered: Vec<_> = if reverse { points.iter().rev().copied().collect() } else { points.clone() };
                for batch_size in [1, 31, 32] {
                    for batch in ordered.chunks(batch_size) {
                        let actual = world.rivers.samples_cached(world.stages(), batch, &mut scratch);
                        let expected: Vec<_> = batch.iter().map(|&(x, z)| world.rivers.sample(world.stages(), x, z, false)).collect();
                        assert_eq!(actual, expected, "batch {batch_size}, seed {seed}, reverse {reverse}");
                        assert!(scratch.corners.len() <= 4096);
                        assert!(scratch.tiers.len() <= 16);
                        assert!(scratch.columns.len() <= 16_384);
                    }
                }
                assert!(world.rivers.samples_cached(world.stages(), &[], &mut scratch).is_empty());
                let split: Vec<_> = ordered[..33].chunks(SAMPLE_BATCH).flat_map(|batch| world.rivers.samples_cached(world.stages(), batch, &mut scratch)).collect();
                let expected: Vec<_> = ordered[..33].iter().map(|&(x, z)| world.rivers.sample(world.stages(), x, z, false)).collect();
                assert_eq!(split, expected, "33-point caller splits into bounded 32+1 batches");
                // Populate valid relief corners near the capacity limit without
                // creating distant river regions, then require a real batch to reset.
                scratch.corners = Corners::default();
                let distant: Vec<_> = (0..900).map(|i| (100_001.0 + 8.0 * i as f32, 100_001.0)).collect();
                world.geology.reliefs_with(&distant, &mut scratch.corners);
                assert!(scratch.corners.len() > 4096 - 20 * SAMPLE_BATCH);
                assert!(scratch.corners.len() <= 4096, "reset fixture starts within the old single-point limit");
                let batch = &ordered[..SAMPLE_BATCH];
                let actual = world.rivers.samples_cached(world.stages(), batch, &mut scratch);
                let expected: Vec<_> = batch.iter().map(|&(x, z)| world.rivers.sample(world.stages(), x, z, false)).collect();
                assert_eq!(actual, expected, "capacity reset, seed {seed}");
                assert!(scratch.corners.len() <= 20 * SAMPLE_BATCH, "whole batch must reserve headroom before centre work");
                assert!(scratch.tiers.len() <= 16);
            }
        }
        assert!(wet > 0 && dry > 0, "fixture needs fresh water and dry ground: {wet}/{dry}");
    }

    #[test]
    fn planning_raw_columns_reuse_neighbours_and_respect_exact_keys() {
        let registry = Arc::new(voxmar_engine::block::BlockRegistry::from_ron("base", include_str!("../../../../assets/content/base/blocks.ron")).unwrap());
        let world = crate::Overworld::new(7, registry.clone());
        let p = (0..625).map(|i| IVec2::new(-1976 + 16 * (i % 25), 552 + 16 * (i / 25)))
            .find(|p| freshwater(&world.rivers.sample(world.stages(), p.x, p.y, false))).expect("actual river fixture");
        let expected = world.rivers.sample(world.stages(), p.x, p.y, false);
        let key = Samples::key(p - 1, p + 1);
        let mut scratch = Samples { id: world.rivers.id, ..Samples::default() };
        let wrong_key = Samples::key(p - REGION, p + REGION);
        assert_ne!(key, wrong_key);
        let wrong = Column { sample: GeoSample { height: 9999, ..expected }, near: true };
        scratch.columns.insert((p, wrong_key), wrong);
        assert_eq!(world.rivers.sample_cached(world.stages(), p.x, p.y, &mut scratch), expected, "same position under different channel bounds cannot alias");
        assert!(scratch.columns[&(p, key)].near);
        assert_eq!(scratch.columns.len(), 6, "exact centre and all four raw neighbours are retained beside the unrelated key");
        let corners = scratch.corners.len();
        assert_eq!(world.rivers.samples_cached(world.stages(), &[(p.x, p.y); 32], &mut scratch), vec![expected; 32]);
        assert_eq!(scratch.columns.len(), 6);
        assert_eq!(scratch.corners.len(), corners, "all warm centres and neighbours need no new terrain");
        // Corner-only eviction must not discard pure raw columns or their keys.
        scratch.corners = Corners::default();
        let distant: Vec<_> = (0..900).map(|i| (100_001.0 + 8.0 * i as f32, 100_001.0)).collect();
        world.geology.reliefs_with(&distant, &mut scratch.corners);
        assert!(scratch.corners.len() > 4096 - 20 * SAMPLE_BATCH);
        assert_eq!(world.rivers.samples_cached(world.stages(), &[(p.x, p.y); 32], &mut scratch), vec![expected; 32]);
        assert_eq!(scratch.corners.len(), 0, "retained raw values survive the corner reset without recomputation");
        // Pressure entries are never queried. Full-batch headroom clears them first.
        scratch.columns.extend((0..16_384 - 6).map(|k| ((IVec2::new(200_000 + k, 200_000), key), wrong)));
        assert_eq!(scratch.columns.len(), 16_384);
        assert_eq!(world.rivers.samples_cached(world.stages(), &[(p.x, p.y); 32], &mut scratch), vec![expected; 32]);
        assert_eq!(scratch.columns.len(), 5);
        scratch.columns.insert((p, key), wrong);
        scratch.columns.insert((p, wrong_key), wrong);
        let another = crate::Overworld::new(42, registry);
        assert_eq!(another.rivers.sample_cached(another.stages(), p.x, p.y, &mut scratch), another.rivers.sample(another.stages(), p.x, p.y, false));
        assert_eq!(scratch.id, another.rivers.id);
        assert!(!scratch.columns.contains_key(&(p, wrong_key)));
        assert!(scratch.columns.len() <= 5);
    }

    #[test]
    fn freshwater_grid_reuses_exact_samples() {
        let registry = Arc::new(voxmar_engine::block::BlockRegistry::from_ron("base", include_str!("../../../../assets/content/base/blocks.ron")).unwrap());
        let mut cache = Freshwater::default();
        for seed in [7, 42, 7] {
            let world = crate::Overworld::new(seed, registry.clone());
            for (first, step, size) in [
                (IVec2::new(-1904, 528), 16, 25),
                (IVec2::new(-1032, -1032), 4, 9),
                (IVec2::new(1008, 1008), 4, 9),
                // Forces grid's >4-region branch, including negative boundaries.
                (IVec2::new(-1200, -1200), 80, 31),
            ] {
                let samples = world.rivers.grid(world.stages(), first, step, size);
                // Poison a distant value and both scratch types as if an earlier
                // world used this same cache, then seed it from the actual grid.
                if cache.id != world.rivers.id {
                    cache.seen.insert(IVec2::splat(200_000), samples[0]);
                    world.rivers.sample_cached(world.stages(), first.x, first.y, &mut cache.scratch);
                    assert!(!cache.scratch.columns.is_empty() && !cache.scratch.tiers.is_empty());
                    world.rivers.remember_freshwater_grid(first, step, size, &samples, &mut cache);
                    assert!(!cache.seen.contains_key(&IVec2::splat(200_000)));
                    assert!(cache.scratch.columns.is_empty() && cache.scratch.tiers.is_empty() && cache.scratch.corners.len() == 0);
                } else {
                    world.rivers.remember_freshwater_grid(first, step, size, &samples, &mut cache);
                }
                assert_eq!(cache.id, world.rivers.id);
                profile_queries(true);
                for (i, &sample) in samples.iter().enumerate() {
                    let p = first + IVec2::new(i as i32 % size, i as i32 / size) * step;
                    assert_eq!(sample, world.rivers.sample(world.stages(), p.x, p.y, false), "seed {seed} point {p:?}");
                    assert_eq!(cache.seen[&p], sample, "complete sealed sample is preserved");
                    assert_eq!(world.rivers.freshwater_near(world.stages(), p, 0, 1, &mut cache), freshwater(&sample));
                }
                let work = query_work().unwrap();
                profile_queries(false);
                assert_eq!(work[3], 0, "every seeded wet and dry centre reuses original terrain");
                assert_eq!(work[6], samples.len());
            }
            // An oversized grid retains only bounded storage and resets before
            // inserting. The retained row prefix must keep the original coordinates.
            let first = IVec2::splat(300_000);
            let samples = vec![cache.seen.values().next().copied().unwrap(); 129 * 129];
            world.rivers.remember_freshwater_grid(first, 1, 129, &samples, &mut cache);
            assert_eq!(cache.seen.len(), 16_384);
            assert_eq!(cache.seen[&(first + IVec2::new(16_383 % 129, 16_383 / 129))], samples[16_383]);
            assert!(!cache.seen.contains_key(&(first + IVec2::new(16_384 % 129, 16_384 / 129))));
        }
    }

    #[test]
    fn freshwater_query_matches_full_grid_circle() {
        let registry = Arc::new(voxmar_engine::block::BlockRegistry::from_ron("base", include_str!("../../../../assets/content/base/blocks.ron")).unwrap());
        let mut wet = 0;
        let mut dry = 0;
        let mut cache = Freshwater::default();
        for (seed, centres) in [
            (7, vec![IVec2::new(-1776, 752), IVec2::new(-1976, 552), IVec2::new(-336, 816)]),
            (11, vec![IVec2::new(-688, 464), IVec2::new(1648, -784)]),
            (42, vec![IVec2::new(720, -784), IVec2::new(-REGION + TRUNK_MARGIN + 1, STREAM_MARGIN - 1)]),
        ] {
            let world = crate::Overworld::new(seed, registry.clone());
            for centre in centres {
                for (radius, step) in [(0, 8), (3, 8), (17, 8), (48, 8), (48, 4)] {
                    let first = centre - radius;
                    let size = radius * 2 / step + 1;
                    let grid = world.rivers.grid(world.stages(), first, step, size);
                    let expected = grid.iter().enumerate().any(|(i, g)| {
                        let p = first + IVec2::new(i as i32 % size, i as i32 / size) * step;
                        p.as_vec2().distance(centre.as_vec2()) <= radius as f32 && freshwater(g)
                    });
                    let actual = world.rivers.freshwater_near(world.stages(), centre, radius, step, &mut cache);
                    assert_eq!(actual, expected, "seed {seed} at {centre}, radius {radius}, step {step}");
                    assert_eq!(cache.id, world.rivers.id);
                    assert!(cache.seen.len() <= 16_384);
                    if expected { wet += 1; } else { dry += 1; }
                }
            }
        }
        assert!(wet > 0 && dry > 0, "query oracle requires positive and negative controls: {wet}/{dry}");
        let world = crate::Overworld::new(7, registry.clone());
        let mut sample = world.rivers.sample(world.stages(), -1776, 752, false);
        sample.height = 140;
        sample.water = 139;
        sample.body = Body::River;
        sample.river = 0.5;
        assert!(!freshwater(&sample));
        sample.river = 0.5f32.next_up();
        assert!(freshwater(&sample));
        sample.body = Body::Sea;
        sample.water = 141;
        assert!(!freshwater(&sample), "even submerged sea is not freshwater");
        let first = IVec2::new(-1976, 552);
        let grid = world.rivers.grid(world.stages(), first, 16, 25);
        let i = grid.iter().position(|g| g.submerged() && g.body != Body::Sea).expect("fixture has actual freshwater");
        let centre = first + IVec2::new(i as i32 % 25, i as i32 / 25) * 16;
        profile_queries(true);
        assert!(world.rivers.freshwater_near(world.stages(), centre, 16, 4, &mut cache));
        let work = query_work().unwrap();
        profile_queries(false);
        assert!(work[3] > 0 && work[3] < work[1], "real freshwater query must save terrain reads: {work:?}");
        assert!(work[2] < work[1], "a positive centre must stop before scanning the full square: {work:?}");
        assert!(work[5] < SAMPLE_BATCH, "last-batch lookahead must be bounded: {work:?}");
        profile_queries(true);
        assert!(world.rivers.freshwater_near(world.stages(), centre, 0, 4, &mut cache));
        let work = query_work().unwrap();
        profile_queries(false);
        assert_eq!(work[3], 0, "a cached exact witness requires no physical terrain reads");
        assert_eq!(work[6], 1);
        // The witness sits after fewer than 32 unknown eligible points. It proves
        // true without flushing the earlier pending positions or counting them as reads.
        cache.seen.clear();
        cache.seen.insert(centre, grid[i]);
        profile_queries(true);
        assert!(world.rivers.freshwater_near(world.stages(), centre, 16, 4, &mut cache));
        let work = query_work().unwrap();
        profile_queries(false);
        assert!(work[2] > 1);
        assert_eq!(work[3], 0, "pending unknown positions are not physical terrain reads");
        assert_eq!(work[4], 0);
        assert_eq!(work[6], 1);
        let another = crate::Overworld::new(42, registry);
        let expected = freshwater(&another.rivers.sample(another.stages(), centre.x, centre.y, false));
        profile_queries(true);
        assert_eq!(another.rivers.freshwater_near(another.stages(), centre, 0, 4, &mut cache), expected);
        let work = query_work().unwrap();
        profile_queries(false);
        assert_eq!(work[3], 1, "another world must recompute even if the Boolean happens to match");
        assert_eq!(cache.id, another.rivers.id);
        assert!(world.rivers.freshwater_near(world.stages(), centre, 0, 4, &mut cache));
        assert_eq!(cache.id, world.rivers.id);
        let dry_i = grid.iter().position(|g| !freshwater(g)).expect("fixture has dry or sea centres");
        let dry = first + IVec2::new(dry_i as i32 % 25, dry_i as i32 / 25) * 16;
        assert!(!world.rivers.freshwater_near(world.stages(), dry, 0, 4, &mut cache));
        profile_queries(true);
        assert!(!world.rivers.freshwater_near(world.stages(), dry, 0, 4, &mut cache));
        let work = query_work().unwrap();
        profile_queries(false);
        assert_eq!(work[3], 0, "a repeated dry query also reuses exact terrain");
        assert_eq!(work[6], 1);
        // Unqueried distant fixture entries create pressure; the actual dry candidate
        // is absent and must be freshly sampled after the whole-map eviction.
        cache.seen.clear();
        cache.seen.extend((0..16_384).map(|k| (IVec2::new(200_000 + k, 200_000), grid[dry_i])));
        assert!(!world.rivers.freshwater_near(world.stages(), dry, 0, 4, &mut cache));
        assert_eq!(cache.seen.len(), 1);
        assert_eq!(cache.seen[&dry], grid[dry_i]);
    }

    /// `cargo test --release -p voxmar_worldgen river_stats -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn river_stats() {
        let seed = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1u64);
        let stream = rng::stream_seed(seed, crate::Overworld::ID, crate::Overworld::VERSION);
        let dials = WorldDials::sample(stream);
        let registry = voxmar_engine::block::BlockRegistry::from_ron("base", include_str!("../../../../assets/content/base/blocks.ron")).unwrap();
        let content = crate::WorldContent::base(&registry).unwrap();
        let rocks = crate::rocks::Rocks::new(stream, &content, &registry, &dials.featured);
        let geology = Geology::new(stream, &dials, &content, &rocks);
        let rivers = Rivers::new(stream, &dials);
        let land = Stages { geology: &geology, climate: geology.climate() };
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
