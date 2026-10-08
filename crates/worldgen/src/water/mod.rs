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

use voxmar_engine::glam::{IVec2, IVec3, Vec3Swizzles};

use self::cache::Cache;
use self::channel::{Carved, Channels, Span, Style, carve, top_block};
use self::drainage::{Budget, DIRS, Drainage, Ground, Reader, TarnRule, Tier, Tile, TunnelRule};
use crate::climate::{self, Climate, LAPSE_RATE, Regional};
use crate::dials::WorldDials;
use fastnoise_lite::{FastNoiseLite, FractalType};

use crate::density::Crag;
use crate::geology::{Corners, GeoSample, Geology, Landform, noise, smoothstep};
use crate::content::WorldContent;
use crate::shapes::{Gate, region_share};
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
    /// Where tarns may lie, if the content has them.
    tarns: Option<Tarns>,
}

/// Where tarns lie (`content::TarnsDef`): their climate, and the slow noise that gathers
/// them into tarn country.
struct Tarns {
    gates: Vec<Gate>,
    country: FastNoiseLite,
    share: f32,
}

impl Tarns {
    /// How fully tarns may lie at a position, 0 ..= 1: none outside tarn country (a sharp
    /// edge, where `region_share` is half way up its ramp: design says none outside, not
    /// fewer), and inside it as far inside their climate as the position lies.
    fn weight(&self, land: Stages, at: IVec2) -> f32 {
        if region_share(self.share, self.country.get_noise_2d(at.x as f32, at.y as f32)) < 0.5 {
            return 0.0;
        }
        let r = land.climate.regional_at(land.geology, at);
        let fields = [r.temperature(), r.moisture(), r.continentalness, r.erosion];
        self.gates.iter().map(|g| g.weight(&fields)).fold(0.0, f32::max)
    }
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

    fn tarns(&self, cell: IVec2) -> f32 {
        let Some(tarns) = &self.rivers.tarns else { return 0.0 };
        let weight = tarns.weight(self.land, self.rivers.streams.tier.centre(cell));
        if weight > 0.0 { weight * self.humidity(cell) } else { 0.0 }
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
    pub fn new(seed: u64, dials: &WorldDials, content: &WorldContent) -> Self {
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
            tarns: None,
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
            tarns: content.tarns.as_ref().map(|t| TarnRule { floor: SEA_LEVEL as f32 + t.floor, depth: t.depth, chance: t.chance }),
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
            tarns: content.tarns.as_ref().map(|t| Tarns {
                gates: t.climate.iter().map(|b| Gate::new(content, b)).collect(),
                country: noise(seed, 0x17, 1.0 / t.country.scale, FractalType::FBm, 2),
                share: t.country.share,
            }),
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
    /// The still lakes whose pits lie in a region of the stream tier, wet ones only: each
    /// one's pit (the centre of its cell, in blocks), top water block and cells. For the
    /// tarn census.
    #[cfg(test)]
    pub(crate) fn stream_lakes(&self, land: Stages, region: IVec2) -> Vec<(IVec2, i32, usize, bool)> {
        let ground = StreamGround { rivers: self, land };
        let basin = self.streams.basin(&ground, region);
        let side = self.streams.tier.side;
        basin
            .lakes
            .iter()
            .filter(|l| l.wet() && !l.spring && l.pit.div_euclid(IVec2::splat(side)) == region)
            .map(|l| (self.streams.tier.centre(l.pit), top_block(l.level), l.cells.len(), l.tarn))
            .collect()
    }

    /// Pits filled by the tarn rule, in the regions within `radius` of `centre`.
    fn tarns_near(&self, land: Stages, centre: IVec2, radius: i32) -> Vec<IVec3> {
        let ground = StreamGround { rivers: self, land };
        let low = (centre - radius).div_euclid(IVec2::splat(REGION));
        let high = (centre + radius).div_euclid(IVec2::splat(REGION));
        let mut found = Vec::new();
        for z in low.y..=high.y {
            for x in low.x..=high.x {
                let region = IVec2::new(x, z);
                for lake in &self.streams.basin(&ground, region).lakes {
                    let pit = self.streams.tier.centre(lake.pit);
                    if lake.tarn && lake.wet() && lake.pit.div_euclid(IVec2::splat(self.streams.tier.side)) == region && (pit - centre).abs().max_element() <= radius {
                        found.push(IVec3::new(pit.x, top_block(lake.level), pit.y));
                    }
                }
            }
        }
        found.sort_unstable_by_key(|p| ((p.xz() - centre).as_i64vec2().length_squared(), p.x, p.z));
        found
    }

    /// The lowest a tarn lies, in blocks, if there are tarns.
    pub(crate) fn tarn_floor(&self) -> Option<f32> {
        self.streams.tier.tarns.as_ref().map(|t| t.floor)
    }

    /// How much of tarn country a position lies in, 0 ..= 1 (none without tarns), for the
    /// tarn census.
    #[cfg(test)]
    pub(crate) fn tarn_country(&self, x: i32, z: i32) -> f32 {
        self.tarns.as_ref().map_or(0.0, |t| region_share(t.share, t.country.get_noise_2d(x as f32, z as f32)))
    }

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

/// The four-neighbour component of lake water nearest a pit. Dry banks cannot bridge
/// pools. The bounded result is a lower bound when water reaches the sampling edge.
pub(crate) fn lake_component(pit: IVec2, reach: i32, wet: impl Fn(IVec2) -> bool) -> Vec<IVec2> {
    let near = reach.min(8);
    let start = (pit.y - near..=pit.y + near).flat_map(|z| (pit.x - near..=pit.x + near).map(move |x| IVec2::new(x, z)))
        .filter(|&p| wet(p)).min_by_key(|p| ((*p - pit).length_squared(), p.x, p.y));
    let Some(start) = start else { return Vec::new() };
    let mut seen = std::collections::HashSet::new();
    let mut todo = vec![start];
    let mut out = Vec::new();
    while let Some(p) = todo.pop() {
        if (p - pit).abs().max_element() > reach || !seen.insert(p) || !wet(p) {
            continue;
        }
        out.push(p);
        todo.extend([p + IVec2::X, p - IVec2::X, p + IVec2::Y, p - IVec2::Y]);
    }
    out.sort_unstable_by_key(|p| (p.x, p.y));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dials::WorldDials;

    /// A dry saddle between same-level lakes does not join them into one visible pool.
    #[test]
    fn lake_components_do_not_cross_dry_banks() {
        let wet = |p: IVec2| p.y == 0 && ((0..=2).contains(&p.x) || (4..=24).contains(&p.x));
        assert_eq!(lake_component(IVec2::ZERO, 32, wet), vec![IVec2::new(0, 0), IVec2::new(1, 0), IVec2::new(2, 0)]);
        assert_eq!(lake_component(IVec2::new(4, 0), 32, wet).len(), 21);
    }

    /// The game gate's seed has a real, visible pool near its nearest mountains. The
    /// query must return the tarn rule's water, and disappear when that rule is absent.
    #[test]
    fn tarn_query_finds_visible_rule_pools() {
        let registry = Arc::new(voxmar_engine::block::BlockRegistry::from_ron("base", include_str!("../../../../assets/content/base/blocks.ron")).unwrap());
        let mut world = crate::Overworld::new(0x5eed, registry);
        let spawn = crate::Generator::spawn_point(&world);
        let mountain = world.locate_biome("mountains", spawn.xz(), 6000).expect("mountains near the gate's spawn");
        println!("gate seed spawn {spawn}, mountain {mountain}");
        let at = world.locate_tarn(mountain, 4096).expect("a visible tarn near those mountains");
        println!("gate seed tarn {at}");
        let wet = lake_component(at.xz(), 16, |p| {
            let g = world.geo(p.x, p.y);
            g.body == Body::Lake && g.submerged() && g.water == at.y
        }).len();
        assert!(wet >= 20, "only {wet} visible water columns at {at}");
        assert!(world.rivers.tarns_near(world.stages(), mountain, 4096).iter().any(|pit| pit.y == at.y && (pit.xz() - at.xz()).abs().max_element() <= 8));
        assert!(world.locate_tarn(mountain, -1).is_none());
        world.rivers.tarns = None;
        assert!(world.locate_tarn(mountain, 4096).is_none(), "an ordinary high lake stood in for a disabled tarn rule");
    }

    /// The country's faded noise is a classification, not a chance multiplier. In the
    /// fading band outside its halfway edge even a fully suitable climate stays dry.
    #[test]
    fn tarn_country_has_a_sharp_edge() {
        let registry = Arc::new(voxmar_engine::block::BlockRegistry::from_ron("base", include_str!("../../../../assets/content/base/blocks.ron")).unwrap());
        let content = crate::WorldContent::base(&registry).unwrap();
        let mut world = crate::Overworld::new(1, registry);
        let tarns = world.rivers.tarns.as_mut().unwrap();
        tarns.gates = vec![Gate::new(&content, &crate::content::ClimateBox::default())];
        let (mut edge, mut inside) = (0, 0);
        for z in (-2048..=2048).step_by(64) {
            for x in (-2048..=2048).step_by(64) {
                let at = IVec2::new(x, z);
                let tarns = world.rivers.tarns.as_ref().unwrap();
                let country = world.rivers.tarn_country(x, z);
                let weight = tarns.weight(world.stages(), at);
                if (0.0..0.5).contains(&country) && country > 0.0 {
                    edge += 1;
                    assert_eq!(weight, 0.0, "tarn chance fades beyond its country at {at}");
                } else if country >= 0.5 {
                    inside += 1;
                    assert_eq!(weight, 1.0, "country weakens a fully suitable climate at {at}");
                }
            }
        }
        assert!(edge > 10 && inside > 10, "sample missed the edge or country: {edge}, {inside}");
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
        let rivers = Rivers::new(stream, &dials, &content);
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
