//! Caves: tunnels, caverns, labyrinths, ravines and sinkholes carved out of the ground, and
//! the water and lava that fill some of them.
//!
//! Every kind of cave has its own rarity, and most are gathered into regions by smooth 2D
//! masks, so some places are riddled with caves and most are not:
//!
//! | Kind | Rarity | Shape |
//! | --- | --- | --- |
//! | Tunnels | common | long winding tubes where two noise fields are both near zero; a third field adds two more families below sea level, so systems grow more tangled with depth (YUNG's Better Caves) |
//! | Entrances | common | tunnels allowed to reach the surface in about a fifth of the land, flaring into a mouth; on flat ground only where the hole leads down, so tunnels never scrape troughs |
//! | Caverns | uncommon | cheese caverns where a noise is high, in cavernous regions, larger with depth, with hourglass pillars |
//! | Noodles | uncommon | thin, claustrophobic tubes, in their own regions |
//! | Underground lakes | uncommon | water tables and perched pools (below) |
//! | Ravines | rare | buried canyons carved along seeded paths, about one per 4 km², 30–70 deep and 3–12 wide, never open to the sky (gorges open to it are cut by rivers, in `crate::water`) |
//! | Labyrinths | rare | mazes of level passages on several floors, domain-warped (Cave Overhaul) |
//! | Sinkholes | rare | shafts from the surface down into the caves, some flooded into cenotes |
//! | Geodes | rare | buried hollows 4–9 blocks across, lined with crystal by `crate::underground` |
//! | Giant caverns | very rare | cavern systems hundreds of blocks across, most holding one great lake whose pillars stand in it as islands |
//! | Long ravines | very rare | buried ravines a kilometre long |
//!
//! The world's cave density dial scales all of them, the world's featured rarity table
//! (`crate::rarity`) boosts some kinds, and the seed varies each and makes one scarce, so
//! worlds differ underground as much as above.
//!
//! Like the rest of generation, every carver is a pure function of the seed and the block
//! position: noise fields are sampled on coarse lattices and interpolated (see
//! [`crate::lattice`]), region masks gate the expensive fields per column, and ravines and
//! sinkholes are seeded per region cell and recomputed by every column they reach.
//!
//! **Water tables** follow Minecraft 1.18's aquifers. Space is cut into cells 16 blocks
//! across and 12 high (their faces warped so they never show as straight lines), and every
//! carved block below its cell's water level fills with water. Cells near the surface take
//! the sea's level, so caves that open under the sea flood instead of draining it. Deeper
//! cells take the level of their basin (a few hundred blocks across): many basins are dry,
//! some flood only their deepest caves and a few flood whole cave systems; most giant cavern
//! regions share one lake level across the whole region. A few cells above
//! the table hold perched pools at their own level. Below `LAVA_LEVEL` caves fill with lava
//! unless water floods them. Wherever neighbouring cells would put a fluid beside air, or
//! water beside lava, a barrier of rock is left in the band where their cells meet, so
//! generated fluids are always contained and never hang in the air.

use fastnoise_lite::{DomainWarpType, FastNoiseLite, FractalType};
use voxmar_engine::coords::CHUNK_SIZE;
use voxmar_engine::glam::{IVec3, Vec2, Vec3};

use crate::SEA_LEVEL;
use crate::dials::WorldDials;
use crate::features;
use crate::geology::{GeoGrid, GeoSample, noise, smoothstep, spline};
use crate::lattice::{Grid, Lattice, Profile};
use crate::rarity::Rarity;
use crate::rng::{Rng, hash3};

/// What the caves leave at a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cave {
    /// Untouched ground, or a barrier that keeps a fluid in.
    Rock,
    Air,
    Water,
    Lava,
}

/// Open caves below this height fill with lava, unless a water table floods them.
pub const LAVA_LEVEL: i32 = 10;
/// Caves are never carved below this height, so the bedrock floor stays whole.
const FLOOR: i32 = 4;

/// Water table cells, in blocks.
const CELL_XZ: i32 = 16;
const CELL_Y: i32 = 12;
/// How far the cell faces are warped, horizontally and vertically.
const WARP_XZ: f32 = 6.0;
const WARP_Y: f32 = 4.0;
/// Blocks on each side of a face between cells whose fluids differ that stay rock. More
/// than one block plus the most a neighbour's warp can differ, so no fluid ever touches
/// air or the other fluid across a face.
const BARRIER: f32 = 2.0;
/// Cells whose top is within this many blocks of the surface take the sea's level.
const SURFACE_BAND: i32 = 18;
/// Basins of one water table, in blocks.
const BASIN: i32 = 160;
/// Water level of a cell with no water.
const DRY: i16 = i16::MIN;

/// Ravines are seeded per region cell of this size.
const RAVINE_CELL: i32 = 1024;
/// Distance between points along a ravine's path.
const RAVINE_STEP: f32 = 8.0;
/// Sinkholes are seeded per cell of this size.
const SINKHOLE_CELL: i32 = 192;
/// Geodes are seeded per cell of this size.
const GEODE_CELL: i32 = 96;
/// Rock around a geode's hollow: crystal, then calcite, then dark rock.
pub(crate) const GEODE_SHELL: f32 = 3.2;

/// Gradient of the 3D noise fields where they cross zero, in field units per block per
/// unit of frequency (measured). Dividing a field by it and its frequency gives roughly
/// the distance to its zero surface in blocks, which is how tube radii are set in blocks.
const GRADIENT: f32 = 2.9;

const STYLE: u64 = 0xca7e;
const RAVINES: u64 = 0x7a71;
const SINKHOLES: u64 = 0x5141;
const GEODES: u64 = 0x6e0d;
const BASINS: u64 = 0xba51;
const PERCHED: u64 = 0x9e7c;
const LEDGES: u64 = 0x1ed6;

/// How much of each kind of cave a world has: multipliers around 1, from the cave dial and
/// the seed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaveStyle {
    pub tunnels: f32,
    pub caverns: f32,
    pub giants: f32,
    pub noodles: f32,
    pub labyrinths: f32,
    pub ravines: f32,
    pub sinkholes: f32,
    pub flooding: f32,
}

impl CaveStyle {
    /// The kinds, in field order, with their rarity (which the base rates in this module
    /// already follow; the world's featured table boosts some of them from it).
    const KINDS: [(&str, Rarity); 8] = [
        ("tunnels", Rarity::Common),
        ("caverns", Rarity::Uncommon),
        ("giant_caverns", Rarity::VeryRare),
        ("noodles", Rarity::Uncommon),
        ("labyrinths", Rarity::Rare),
        ("ravines", Rarity::Rare),
        ("sinkholes", Rarity::Rare),
        ("flooded_caves", Rarity::Uncommon),
    ];
    /// The most a featured boost multiplies a kind by: a tier's jump is too much for things
    /// as large as caves.
    const MOST_FEATURED: f32 = 4.0;

    fn sample(seed: u64, dials: &WorldDials) -> Self {
        let mut rng = Rng::new(seed, STYLE, 0, 0);
        let amount = 0.45 + 1.1 * dials.cave_density;
        // Each kind varies by up to 1.7× either way, the world features some (see
        // `crate::rarity`), and one kind is scarce.
        let featured = dials.featured.multipliers("cave", &Self::KINDS);
        let mut kinds: [f32; 8] = std::array::from_fn(|i| 2f32.powf(1.5 * (rng.unit() - 0.5)) * featured[i].min(Self::MOST_FEATURED));
        kinds[rng.below(8) as usize] *= 0.4;
        let [tunnels, caverns, giants, noodles, labyrinths, ravines, sinkholes, flooding] = kinds;
        Self {
            // Tunnels are what most caving happens in, so they vary least.
            tunnels: (amount * tunnels).sqrt(),
            caverns: amount * caverns,
            giants: amount * giants,
            noodles: amount * noodles,
            labyrinths: amount * labyrinths,
            ravines: amount * ravines,
            sinkholes: amount * sinkholes,
            flooding,
        }
    }
}

/// The value a 2D two-octave noise field exceeds over `share` of the world (measured).
fn cut_for_share(share: f32) -> f32 {
    spline(
        &[(0.002, 0.9), (0.01, 0.804), (0.03, 0.719), (0.05, 0.659), (0.1, 0.547), (0.2, 0.381), (0.3, 0.248), (0.5, 0.0), (0.8, -0.381)],
        share,
    )
}

/// The cave carvers of one world.
pub struct Caves {
    style: CaveStyle,
    heights: Heights,
    seed: u64,
    /// Water level of the lakes in giant caverns, the same across the world so that one
    /// lake never splits at a basin border.
    lake_level: i16,
    /// Where each kind is found: 2D masks and the values above which they appear.
    cavern_regions: FastNoiseLite,
    cavern_cut: f32,
    giant_regions: FastNoiseLite,
    giant_cut: f32,
    /// Which giant cavern regions are flooded.
    lake_regions: FastNoiseLite,
    noodle_regions: FastNoiseLite,
    noodle_cut: f32,
    labyrinth_regions: FastNoiseLite,
    labyrinth_cut: f32,
    entrances: FastNoiseLite,
    entrance_cut: f32,
    tunnel_width: FastNoiseLite,
    pillars: FastNoiseLite,
    /// Shapes.
    tunnels: [FastNoiseLite; 3],
    cheese: FastNoiseLite,
    giant: FastNoiseLite,
    noodles: [FastNoiseLite; 2],
    labyrinth: [FastNoiseLite; 2],
    labyrinth_warp: FastNoiseLite,
    /// Warps the faces of the water table cells.
    table_warp: [FastNoiseLite; 3],
}

/// Tunnel noise frequency, and how strongly heights are scaled (flattening the tubes).
const TUNNEL_FREQUENCY: f32 = 1.0 / 125.0;
const TUNNEL_SCALE_Y: f32 = 1.3;
const NOODLE_FREQUENCY: f32 = 1.0 / 40.0;
/// Labyrinths are mazes of level passages on several floors: where the zero sheets of a
/// field stretched upright (so its sheets stand like walls) cross bands of floor height
/// (a wave in y, wobbling with a second field). A level cut through a floor shows the
/// first field's whole zero contour, a winding network with dead ends.
const LABYRINTH_FREQUENCY: f32 = 1.0 / 30.0;
const LABYRINTH_SCALE_Y: f32 = 0.3;
/// Height of one floor of a labyrinth, and the half-height of its passages in units of the
/// floor wave (|sin| below this is about 4 blocks of the 14).
const LABYRINTH_FLOOR: f32 = 14.0;
const LABYRINTH_PASSAGE: f32 = 0.42;

impl Caves {
    pub fn new(seed: u64, dials: &WorldDials) -> Self {
        let style = CaveStyle::sample(seed, dials);
        let mut warp = noise(seed, 47, 1.0 / 64.0, FractalType::DomainWarpIndependent, 1);
        warp.set_domain_warp_type(Some(DomainWarpType::OpenSimplex2));
        warp.set_domain_warp_amp(Some(22.0));
        let mut rng = Rng::new(seed, STYLE, 1, 0);
        Self {
            style,
            heights: Heights::new(),
            seed,
            lake_level: (SEA_LEVEL - 76 + rng.below(28)) as i16,
            cavern_regions: noise(seed, 30, 1.0 / 900.0, FractalType::FBm, 2),
            cavern_cut: cut_for_share((0.2 * style.caverns).clamp(0.03, 0.55)),
            giant_regions: noise(seed, 31, 1.0 / 2800.0, FractalType::FBm, 2),
            giant_cut: cut_for_share((0.025 * style.giants).clamp(0.003, 0.1)),
            lake_regions: noise(seed, 51, 1.0 / 7000.0, FractalType::None, 1),
            noodle_regions: noise(seed, 32, 1.0 / 500.0, FractalType::FBm, 2),
            noodle_cut: cut_for_share((0.25 * style.noodles).clamp(0.03, 0.6)),
            labyrinth_regions: noise(seed, 33, 1.0 / 1600.0, FractalType::FBm, 2),
            labyrinth_cut: cut_for_share((0.05 * style.labyrinths).clamp(0.005, 0.2)),
            entrances: noise(seed, 34, 1.0 / 260.0, FractalType::FBm, 2),
            entrance_cut: cut_for_share((0.2 * style.tunnels).clamp(0.05, 0.4)),
            tunnel_width: noise(seed, 35, 1.0 / 300.0, FractalType::FBm, 2),
            pillars: noise(seed, 36, 1.0 / 26.0, FractalType::None, 1),
            tunnels: [37, 38, 39].map(|salt| noise(seed, salt, TUNNEL_FREQUENCY, FractalType::FBm, 2)),
            cheese: noise(seed, 40, 1.0 / 140.0, FractalType::FBm, 2),
            giant: noise(seed, 41, 1.0 / 260.0, FractalType::FBm, 2),
            noodles: [42, 43].map(|salt| noise(seed, salt, NOODLE_FREQUENCY, FractalType::None, 1)),
            labyrinth: [44, 45].map(|salt| noise(seed, salt, LABYRINTH_FREQUENCY, FractalType::None, 1)),
            labyrinth_warp: warp,
            table_warp: [48, 49, 50].map(|salt| noise(seed, salt, 1.0 / 90.0, FractalType::None, 1)),
        }
    }

    pub fn style(&self) -> &CaveStyle {
        &self.style
    }

    /// Radius of the main tunnels in field units at height `y`, `depth` under the surface,
    /// given the local width and entrance fields.
    #[inline]
    fn tunnel_radius(&self, width: f32, entrance: f32, y: i32, depth: i32) -> f32 {
        self.tunnel_scale(width) * tunnel_growth(y) * tunnel_fade(entrance, depth)
    }

    /// The part of the tunnel radius that depends only on the column.
    #[inline]
    fn tunnel_scale(&self, width: f32) -> f32 {
        1.8 * self.style.tunnels * (1.0 + 0.4 * width) * GRADIENT * TUNNEL_FREQUENCY
    }

    /// Blocks under a river or lake bed, and under its banks, kept clear of caves, so they
    /// never drain it. A river's own tunnel through a range is carved by the rivers.
    fn clearance(geo: &GeoSample) -> i32 {
        // Under water, and under banks not far above the water beside them, whatever its
        // height: a river or lake in the hills needs the band as much as the sea does.
        if geo.submerged() || geo.cavity.is_some() || (geo.river > 0.05 && geo.height - geo.water <= 4) {
            return 10;
        }
        (10.0 * smoothstep(0.15, 0.5, geo.river)) as i32
    }

    /// Prepares the caves of the column at `origin`, whose ground (with a one-block border)
    /// `geology` holds, up to height `top`.
    pub fn column<'a>(&'a self, geology: &'a GeoGrid<'a>, origin: IVec3, top: i32) -> CaveColumn<'a> {
        let _span = tracing::info_span!("caves").entered();
        let grid = |n: &FastNoiseLite| Grid::noise(n, origin);
        let caverns = grid(&self.cavern_regions);
        let giants = grid(&self.giant_regions);
        let noodle_regions = grid(&self.noodle_regions);
        let labyrinth_regions = grid(&self.labyrinth_regions);
        let low = FLOOR;
        // Carvers stop at the ground, so their lattices stop at the highest ground (water
        // and the sea can stand above it).
        let ground = geology.samples().iter().map(|g| g.height).max().unwrap_or(0).min(top);
        let lattice = |n: &FastNoiseLite, high: i32, step_y: i32, scale_y: f32| Lattice::noise(n, origin, low, high.max(low), step_y, scale_y);
        let tunnels = [
            lattice(&self.tunnels[0], ground, 8, TUNNEL_SCALE_Y),
            lattice(&self.tunnels[1], ground, 8, TUNNEL_SCALE_Y),
            lattice(&self.tunnels[2], ground.min(DEEP_TUNNELS), 8, TUNNEL_SCALE_Y),
        ];
        let cavernous = caverns.max() > self.cavern_cut || giants.max() > self.giant_cut;
        let cheese = cavernous.then(|| lattice(&self.cheese, ground, 8, 1.7));
        let giant = (giants.max() > self.giant_cut).then(|| lattice(&self.giant, ground, 8, 1.5));
        let noodles = (noodle_regions.max() > self.noodle_cut)
            .then(|| [0, 1].map(|i| lattice(&self.noodles[i], ground, 4, 1.0)));
        let labyrinth = (labyrinth_regions.max() > self.labyrinth_cut).then(|| {
            [0, 1].map(|i| {
                Lattice::new(origin, low, ground.max(low), 4, |p| {
                    let (x, y, z) = self.labyrinth_warp.domain_warp_3d(p.x as f32, p.y as f32, p.z as f32);
                    if i == 0 {
                        self.labyrinth[0].get_noise_3d(x, y * LABYRINTH_SCALE_Y, z)
                    } else {
                        let wobble = 4.0 * self.labyrinth[1].get_noise_3d(x, y, z);
                        ((p.y as f32 + wobble) * std::f32::consts::PI / LABYRINTH_FLOOR).sin()
                    }
                })
            })
        });
        CaveColumn {
            caves: self,
            geology,
            origin,
            caverns,
            giants,
            noodle_regions,
            labyrinth_regions,
            entrances: grid(&self.entrances),
            width: grid(&self.tunnel_width),
            warp: self.table_warp.each_ref().map(grid),
            tunnels,
            cheese,
            giant,
            noodles,
            labyrinth,
            ravines: self.ravines_near(geology, origin),
            sinkholes: self.sinkholes_near(geology, origin),
            geodes: self.geodes_near(geology, origin),
            tables: Tables::new(self, geology, origin, top),
            profiles: Default::default(),
        }
    }

    /// Water table cells' levels: water fills carved blocks below it.
    fn cell_level(&self, cell: IVec3, surface: i32, basin: i16) -> i16 {
        let bottom = cell.y * CELL_Y;
        if bottom + CELL_Y > surface - SURFACE_BAND {
            return (SEA_LEVEL + 1) as i16;
        }
        // A few dry cells hold a perched pool at their own level (always above the lava, so
        // water never lies under it).
        let hash = hash3(self.seed ^ PERCHED, cell.x, cell.y, cell.z);
        let pool = (hash & 0xffff) as f32 / 65536.0;
        // Not in caverns, where they would leave rock bowls hanging in the open.
        let centre = ((cell.x * CELL_XZ + CELL_XZ / 2) as f32, (cell.z * CELL_XZ + CELL_XZ / 2) as f32);
        let perched = pool < 0.04 * self.style.flooding.min(2.0)
            && (bottom as i16) > basin
            && bottom > LAVA_LEVEL
            && bottom + CELL_Y < surface - SURFACE_BAND - 8
            && self.cavern_regions.get_noise_2d(centre.0, centre.1) < self.cavern_cut - 0.05
            && self.giant_regions.get_noise_2d(centre.0, centre.1) < self.giant_cut - 0.05;
        if perched {
            return (bottom + 3 + (hash >> 16) as i32 % 6) as i16;
        }
        basin
    }

    /// The water table of the basin around (x, z). Basins are the cells of a jittered grid
    /// (each position belongs to the nearest basin centre), so their borders wander.
    fn basin_level(&self, x: i32, z: i32) -> i16 {
        // Most giant cavern regions hold one great lake, whose shore follows the region's
        // edge (just outside it, so the cavern never shows a dam).
        let (xf, zf) = (x as f32, z as f32);
        if self.giant_regions.get_noise_2d(xf, zf) > self.giant_cut - 0.03 && self.lake_regions.get_noise_2d(xf, zf) > -0.25 {
            return self.lake_level;
        }
        let (cx, cz) = (x.div_euclid(BASIN), z.div_euclid(BASIN));
        let centre = |bx: i32, bz: i32| {
            let mut rng = Rng::new(self.seed, BASINS, bx, bz);
            let jitter = Vec2::new(rng.unit(), rng.unit()) * 0.8 + 0.1;
            (Vec2::new(bx as f32, bz as f32) + jitter) * BASIN as f32
        };
        let p = Vec2::new(x as f32, z as f32);
        let (bx, bz) = (-1..=1)
            .flat_map(|dz| (-1..=1).map(move |dx| (cx + dx, cz + dz)))
            .min_by(|a, b| p.distance_squared(centre(a.0, a.1)).total_cmp(&p.distance_squared(centre(b.0, b.1))))
            .unwrap();
        let mut rng = Rng::new(self.seed, BASINS ^ 1, bx, bz);
        let wet = (0.32 * self.style.flooding).clamp(0.08, 0.75);
        let roll = rng.unit();
        if roll >= wet {
            DRY
        } else if roll < wet * 0.75 {
            // Only the deepest caves flood.
            (LAVA_LEVEL + 4 + rng.below(30)) as i16
        } else {
            // Whole cave systems flood.
            (50 + rng.below((SEA_LEVEL - 82) as u32)) as i16
        }
    }

    /// Ravines whose path passes within reach of the column at `origin`.
    fn ravines_near(&self, geology: &GeoGrid, origin: IVec3) -> Vec<Ravine> {
        let min = Vec2::new(origin.x as f32, origin.z as f32);
        let max = min + CHUNK_SIZE as f32;
        let cell = |v: i32| v.div_euclid(RAVINE_CELL);
        let mut found = Vec::new();
        // Paths reach at most 1300 blocks from their start.
        for rz in cell(origin.z - 1300)..=cell(origin.z + CHUNK_SIZE + 1300) {
            for rx in cell(origin.x - 1300)..=cell(origin.x + CHUNK_SIZE + 1300) {
                if let Some(path) = self.ravine_path(rx, rz)
                    && let Some(ravine) = Ravine::new(path, geology, min, max)
                {
                    found.push(ravine);
                }
            }
        }
        found
    }

    /// The path of the ravine seeded in region cell (rx, rz), if it has one.
    fn ravine_path(&self, rx: i32, rz: i32) -> Option<RavinePath> {
        let mut rng = Rng::new(self.seed, RAVINES, rx, rz);
        if !rng.chance(0.25 * self.style.ravines) {
            return None;
        }
        let start = Vec2::new((rx * RAVINE_CELL) as f32, (rz * RAVINE_CELL) as f32) + Vec2::new(rng.unit(), rng.unit()) * RAVINE_CELL as f32;
        // A few run for a kilometre.
        let length = if rng.chance(0.08) { 700.0 + 550.0 * rng.unit() } else { 140.0 + 300.0 * rng.unit() };
        let mut heading = rng.unit() * std::f32::consts::TAU;
        let mut turn = 0.0f32;
        let mut at = start;
        let points = (0..=(length / RAVINE_STEP) as usize)
            .map(|_| {
                let here = at;
                turn = (turn * 0.92 + (rng.unit() - 0.5) * 0.05).clamp(-0.07, 0.07);
                heading += turn;
                at += Vec2::from_angle(heading) * RAVINE_STEP;
                here
            })
            .collect();
        Some(RavinePath {
            points,
            depth: 30.0 + 40.0 * rng.unit(),
            width: 3.0 + 9.0 * rng.unit(),
            // Buried, always: gorges open to the sky are cut by rivers (`crate::water`),
            // where water would cut them. (The roll that once opened some is still drawn,
            // so the values after it stay as they were.)
            cover: {
                let _ = rng.chance(0.45);
                6.0 + 14.0 * rng.unit()
            },
            phases: [rng.unit(), rng.unit()].map(|u| u * std::f32::consts::TAU),
            ledges: rng.next_u64(),
        })
    }

    /// Geodes within reach of the column at `origin`: hollow balls of crystal buried in
    /// the rock, found by mining into them or where a cave cuts one open. Their hollows are
    /// carved like caves (so the water tables flood some); `crate::underground` lines them.
    pub(crate) fn geodes_near(&self, geology: &GeoGrid, origin: IVec3) -> Vec<Geode> {
        const REACH: i32 = 9;
        let cell = |v: i32| v.div_euclid(GEODE_CELL);
        let mut found = Vec::new();
        for gz in cell(origin.z - REACH)..=cell(origin.z + CHUNK_SIZE + REACH) {
            for gx in cell(origin.x - REACH)..=cell(origin.x + CHUNK_SIZE + REACH) {
                let mut rng = Rng::new(self.seed, GEODES, gx, gz);
                if !rng.chance(0.12) {
                    continue;
                }
                let span = (GEODE_CELL - 2 * REACH) as u32;
                let (x, z) = (gx * GEODE_CELL + REACH + rng.below(span), gz * GEODE_CELL + REACH + rng.below(span));
                let near = |c: i32, o: i32| (o - REACH..o + CHUNK_SIZE + REACH).contains(&c);
                if !near(x, origin.x) || !near(z, origin.z) {
                    continue;
                }
                let y = rng.range(LAVA_LEVEL + 12, SEA_LEVEL - 20);
                let radius = 2.2 + 2.3 * rng.unit();
                // Well inside the ground, clear of river beds, and not under the sea or a
                // lake, whose water tables would wall the hollow up.
                let ground = geology.get(x, z);
                if ground.height - y < 18 || ground.submerged() {
                    continue;
                }
                found.push(Geode { centre: Vec3::new(x as f32, y as f32, z as f32) + 0.5, radius, seed: rng.next_u64() });
            }
        }
        found
    }

    /// Sinkholes within reach of the column at `origin`.
    fn sinkholes_near(&self, geology: &GeoGrid, origin: IVec3) -> Vec<Sinkhole> {
        // The widest sinkhole's rim, plus the reach of features asking `surface_open` (and a
        // column more, for the columns beside a candidate they ask about too).
        const REACH: i32 = 32 + features::REACH + 1;
        let cell = |v: i32| v.div_euclid(SINKHOLE_CELL);
        let mut found = Vec::new();
        for sz in cell(origin.z - REACH)..=cell(origin.z + CHUNK_SIZE + REACH) {
            for sx in cell(origin.x - REACH)..=cell(origin.x + CHUNK_SIZE + REACH) {
                let mut rng = Rng::new(self.seed, SINKHOLES, sx, sz);
                if !rng.chance(0.035 * self.style.sinkholes) {
                    continue;
                }
                let margin = REACH;
                let span = (SINKHOLE_CELL - 2 * margin) as u32;
                let centre = IVec3::new(sx * SINKHOLE_CELL + margin + rng.below(span), 0, sz * SINKHOLE_CELL + margin + rng.below(span));
                let near = |c: i32, o: i32| (o - REACH..o + CHUNK_SIZE + REACH).contains(&c);
                if !near(centre.x, origin.x) || !near(centre.z, origin.z) {
                    continue;
                }
                let geo = geology.get(centre.x, centre.z);
                if geo.height < SEA_LEVEL + 3 || geo.river > 0.1 {
                    continue;
                }
                let great = rng.chance(0.06);
                found.push(Sinkhole {
                    centre: Vec2::new(centre.x as f32 + 0.5, centre.z as f32 + 0.5),
                    radius: if great { 10.0 + 6.0 * rng.unit() } else { 3.0 + 6.0 * rng.unit() },
                    surface: geo.height,
                    bottom: geo.height - 18 - rng.below(40),
                    phases: [rng.unit(), rng.unit()].map(|u| u * std::f32::consts::TAU),
                });
            }
        }
        found
    }
}

/// How deep an opening in flat ground must lead.
const MOUTH: i32 = 6;

/// Below this height the third tunnel field joins in.
const DEEP_TUNNELS: i32 = SEA_LEVEL - 8;

/// How much deeper than sea level a height lies, 0 ..= 1 at 40.
#[inline]
fn deepness(y: i32) -> f32 {
    smoothstep((SEA_LEVEL - 10) as f32, 40.0, y as f32)
}

/// Tunnels widen with depth.
#[inline]
fn tunnel_growth(y: i32) -> f32 {
    1.0 + 0.45 * deepness(y)
}

/// Tunnels close before the surface, except at entrances, which flare.
#[inline]
fn tunnel_fade(entrance: f32, depth: i32) -> f32 {
    if depth >= 10 {
        return 1.0;
    }
    let near_surface = smoothstep(1.0, 9.0, depth as f32);
    let mouth = 1.0 + 1.2 * (1.0 - smoothstep(0.0, 10.0, depth as f32));
    near_surface + (mouth - near_surface) * entrance
}

/// Per-height factors shared by every block column, computed once per world.
struct Heights {
    growth: Vec<f32>,
    deep: Vec<f32>,
    /// Radius of the deep tunnel families relative to the main ones.
    more: Vec<f32>,
}

impl Heights {
    fn new() -> Self {
        let ys = 0..voxmar_engine::coords::WORLD_HEIGHT;
        Self {
            growth: ys.clone().map(tunnel_growth).collect(),
            deep: ys.clone().map(deepness).collect(),
            more: ys.map(|y| 0.7 * smoothstep((DEEP_TUNNELS - 2) as f32, (DEEP_TUNNELS - 60) as f32, y as f32)).collect(),
        }
    }
}

/// A ravine's path before it is fitted to the ground.
struct RavinePath {
    points: Vec<Vec2>,
    depth: f32,
    /// Widest full width, in blocks.
    width: f32,
    /// How far under the lowest ground along it the ravine's top lies.
    cover: f32,
    phases: [f32; 2],
    ledges: u64,
}

/// One point of a ravine's centre line, with the shape of its cross-section there.
#[derive(Clone, Copy)]
struct RavinePoint {
    at: Vec2,
    floor: f32,
    top: f32,
    half_width: f32,
}

/// A ravine near one column: the parts of its path that reach it.
struct Ravine {
    points: Vec<RavinePoint>,
    /// Segments (indices of their first point) that pass within reach of the column.
    segments: Vec<usize>,
    ledges: u64,
}

impl Ravine {
    /// Fits the path to the ground, if it passes within reach of the column from `min` to
    /// `max`.
    fn new(path: RavinePath, geology: &GeoGrid, min: Vec2, max: Vec2) -> Option<Self> {
        // As far as the ravine's walls reach, plus the reach of features asking
        // `surface_open`.
        let reach = path.width * 0.75 + 2.0 + features::REACH as f32;
        let (low, high) = (min - reach, max + reach);
        let touches = |a: Vec2, b: Vec2| a.min(b).cmple(high).all() && a.max(b).cmpge(low).all();
        let segments: Vec<usize> = path.points.windows(2).enumerate().filter(|(_, w)| touches(w[0], w[1])).map(|(i, _)| i).collect();
        if segments.is_empty() {
            return None;
        }
        // The lowest ground near each point along the path, so the ravine stays buried
        // where valleys cross it, and its floor does not copy every bump.
        let ground: Vec<f32> = path.points.iter().map(|p| geology.get(p.x as i32, p.y as i32).height as f32).collect();
        let n = ground.len();
        let low = |i: usize| {
            let (a, b) = (i.saturating_sub(3), (i + 3).min(n - 1));
            ground[a..=b].iter().copied().fold(f32::MAX, f32::min)
        };
        let length = (n - 1) as f32 * RAVINE_STEP;
        let points = (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                // Narrow and shallow at both ends, wavering in width along the way.
                let ends = (std::f32::consts::PI * t).sin();
                let s = t * length;
                let waver = 1.0 + 0.25 * (s / 37.0 + path.phases[0]).sin() + 0.15 * (s / 13.0 + path.phases[1]).sin();
                let half_width = 0.5 * path.width * ends.powf(0.6) * waver;
                let depth = path.depth * (0.3 + 0.7 * ends.powf(0.5));
                let top = low(i) - path.cover;
                let floor = top - depth;
                RavinePoint { at: path.points[i], floor: floor.max((LAVA_LEVEL + 2) as f32), top, half_width }
            })
            .collect();
        Some(Self { points, segments, ledges: path.ledges })
    }

    /// The cross-section nearest the block column at `p`, and its distance.
    fn nearest(&self, p: Vec2) -> (f32, RavinePoint) {
        let mut best = (f32::MAX, self.points[0]);
        for &i in &self.segments {
            let (a, b) = (self.points[i], self.points[i + 1]);
            let ab = b.at - a.at;
            let t = ((p - a.at).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
            let distance = p.distance(a.at + ab * t);
            if distance < best.0 {
                let mix = |u: f32, v: f32| u + (v - u) * t;
                best = (distance, RavinePoint {
                    at: a.at + ab * t,
                    floor: mix(a.floor, b.floor),
                    top: mix(a.top, b.top),
                    half_width: mix(a.half_width, b.half_width),
                });
            }
        }
        best
    }
}

/// A ravine's cross-section over one block column.
struct RavineSpan {
    distance: f32,
    point: RavinePoint,
    ledges: u64,
}

impl RavineSpan {
    #[inline]
    fn contains(&self, y: i32) -> bool {
        let p = &self.point;
        let yf = y as f32 + 0.5;
        if yf < p.floor || self.distance >= p.half_width * 1.25 {
            return false;
        }
        let u = (yf - p.floor) / (p.top - p.floor);
        // Lens-shaped, widest halfway up.
        let shape = (1.0 - (2.0 * u - 1.0).powi(2)).max(0.0).sqrt();
        // Ledges: the walls step in and out every couple of blocks, the same along the length.
        let ledge = 1.0 + 0.25 * ((hash3(self.ledges ^ LEDGES, 0, y.div_euclid(2), 0) & 0xff) as f32 / 255.0 - 0.5);
        self.distance < p.half_width * shape * ledge
    }
}

/// A geode: a hollow `radius` blocks across, lined with `GEODE_SHELL` blocks of rock.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Geode {
    pub centre: Vec3,
    pub radius: f32,
    pub seed: u64,
}

impl Geode {
    /// Distance from the centre to the middle of the block at `p`.
    #[inline]
    pub fn distance(&self, p: IVec3) -> f32 {
        (p.as_vec3() + 0.5).distance(self.centre)
    }

    /// The heights the geode spans in the block column at (x, z), with its shell.
    pub fn span(&self, x: i32, z: i32) -> Option<std::ops::RangeInclusive<i32>> {
        let outer = self.radius + GEODE_SHELL;
        let d = Vec2::new(x as f32 + 0.5 - self.centre.x, z as f32 + 0.5 - self.centre.z).length();
        (d < outer).then(|| {
            let half = (outer * outer - d * d).sqrt();
            (self.centre.y - half).floor() as i32..=(self.centre.y + half).ceil() as i32
        })
    }
}

/// A shaft from the surface down into the ground.
struct Sinkhole {
    centre: Vec2,
    radius: f32,
    surface: i32,
    bottom: i32,
    phases: [f32; 2],
}

/// Water levels of the water table cells around one column.
struct Tables {
    /// Cell coordinates of the first entry.
    min: IVec3,
    size: IVec3,
    levels: Vec<i16>,
    /// The highest level among each cell and its 26 neighbours: at or above it, and above
    /// `LAVA_LEVEL`, every cell nearby is dry, so a carved block is plain air.
    reach: Vec<i16>,
}

impl Tables {
    fn new(caves: &Caves, geology: &GeoGrid, origin: IVec3, top: i32) -> Self {
        let warp = IVec3::new(WARP_XZ.ceil() as i32, WARP_Y.ceil() as i32, WARP_XZ.ceil() as i32);
        let low = (origin.with_y(FLOOR) - warp).div_euclid(IVec3::new(CELL_XZ, CELL_Y, CELL_XZ)) - 1;
        let high = (origin + IVec3::new(CHUNK_SIZE - 1, top, CHUNK_SIZE - 1) + warp).div_euclid(IVec3::new(CELL_XZ, CELL_Y, CELL_XZ)) + 1;
        let size = high - low + 1;
        let mut levels = Vec::with_capacity((size.x * size.y * size.z) as usize);
        let mut columns = Vec::with_capacity((size.x * size.z) as usize);
        for k in 0..size.z {
            for i in 0..size.x {
                let (x, z) = ((low.x + i) * CELL_XZ + CELL_XZ / 2, (low.z + k) * CELL_XZ + CELL_XZ / 2);
                columns.push((geology.get(x, z).height, caves.basin_level(x, z)));
            }
        }
        for j in 0..size.y {
            for k in 0..size.z {
                for i in 0..size.x {
                    let (surface, basin) = columns[(k * size.x + i) as usize];
                    levels.push(caves.cell_level(low + IVec3::new(i, j, k), surface, basin));
                }
            }
        }
        let index = |c: IVec3| ((c.y * size.z + c.z) * size.x + c.x) as usize;
        let mut reach = vec![DRY; levels.len()];
        for j in 0..size.y {
            for k in 0..size.z {
                for i in 0..size.x {
                    let mut highest = DRY;
                    for dy in -1..=1 {
                        for dz in -1..=1 {
                            for dx in -1..=1 {
                                let c = IVec3::new(i + dx, j + dy, k + dz).clamp(IVec3::ZERO, size - 1);
                                highest = highest.max(levels[index(c)]);
                            }
                        }
                    }
                    reach[index(IVec3::new(i, j, k))] = highest;
                }
            }
        }
        Self { min: low, size, levels, reach }
    }

    #[inline]
    fn index(&self, cell: IVec3) -> usize {
        let c = cell - self.min;
        debug_assert!(c.cmpge(IVec3::ZERO).all() && c.cmplt(self.size).all(), "cell {cell} outside the column's tables");
        ((c.y * self.size.z + c.z) * self.size.x + c.x) as usize
    }

    /// What a carved block at height `y` holds in a cell with water `level`.
    #[inline]
    fn state(level: i16, y: i32) -> Cave {
        if y < level as i32 {
            Cave::Water
        } else if y < LAVA_LEVEL {
            Cave::Lava
        } else {
            Cave::Air
        }
    }

    /// What a carved block at height `y`, whose warped position is `p`, holds: its cell's
    /// fluid, or rock where a neighbouring cell close by holds something else.
    fn fluid(&self, p: Vec3, y: i32) -> Cave {
        let size = Vec3::new(CELL_XZ as f32, CELL_Y as f32, CELL_XZ as f32);
        let cell = (p / size).floor();
        let index = self.index(cell.as_ivec3());
        if y >= self.reach[index] as i32 && y >= LAVA_LEVEL {
            return Cave::Air;
        }
        let own = Self::state(self.levels[index], y);
        let within = p - cell * size;
        let near = |axis: usize| -> (i32, i32) {
            (if within[axis] < BARRIER { -1 } else { 0 }, if size[axis] - within[axis] < BARRIER { 1 } else { 0 })
        };
        let (nx, ny, nz) = (near(0), near(1), near(2));
        for dy in ny.0..=ny.1 {
            for dz in nz.0..=nz.1 {
                for dx in nx.0..=nx.1 {
                    let other = self.levels[self.index(cell.as_ivec3() + IVec3::new(dx, dy, dz))];
                    if Self::state(other, y) != own {
                        return Cave::Rock;
                    }
                }
            }
        }
        own
    }
}

/// Per-column scratch space for interpolated noise above one block column.
#[derive(Default)]
struct Profiles {
    tunnels: [Profile; 3],
    cheese: Profile,
    giant: Profile,
    noodles: [Profile; 2],
    labyrinth: [Profile; 2],
    /// The tunnel fields at every height of a block column, from `FLOOR`.
    rows: [Vec<f32>; 3],
}

/// The caves of one column, ready to carve block column by block column.
pub struct CaveColumn<'a> {
    caves: &'a Caves,
    geology: &'a GeoGrid<'a>,
    origin: IVec3,
    caverns: Grid,
    giants: Grid,
    noodle_regions: Grid,
    labyrinth_regions: Grid,
    entrances: Grid,
    width: Grid,
    warp: [Grid; 3],
    tunnels: [Lattice; 3],
    cheese: Option<Lattice>,
    giant: Option<Lattice>,
    noodles: Option<[Lattice; 2]>,
    labyrinth: Option<[Lattice; 2]>,
    ravines: Vec<Ravine>,
    sinkholes: Vec<Sinkhole>,
    geodes: Vec<Geode>,
    tables: Tables,
    profiles: Box<Profiles>,
}

impl CaveColumn<'_> {
    /// Fills `out` with what the caves leave at every height from 0 to the surface of the
    /// block column at (x, z), relative to the column origin.
    pub fn carve(&mut self, x: i32, z: i32, out: &mut Vec<Cave>) {
        let caves = self.caves;
        let (wx, wz) = (self.origin.x + x, self.origin.z + z);
        let geo = self.geology.get(wx, wz);
        let height = geo.height;
        out.clear();
        out.resize(height as usize + 1, Cave::Rock);
        if height < FLOOR {
            return;
        }

        // Region strengths: 0 outside a kind's regions, rising to 1 inside.
        let cavern = smoothstep(caves.cavern_cut, caves.cavern_cut + 0.15, self.caverns.at(x, z));
        let giant = smoothstep(caves.giant_cut, caves.giant_cut + 0.08, self.giants.at(x, z));
        let noodle = smoothstep(caves.noodle_cut, caves.noodle_cut + 0.12, self.noodle_regions.at(x, z));
        let labyrinth = smoothstep(caves.labyrinth_cut, caves.labyrinth_cut + 0.08, self.labyrinth_regions.at(x, z));
        let entrance = smoothstep(caves.entrance_cut, caves.entrance_cut + 0.1, self.entrances.at(x, z));
        // Beside water, rock all the way down past its bed, however deep the water lies
        // below this column's ground (the wall of a gorge a creek runs in).
        let beside = [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .into_iter()
            .map(|(dx, dz)| self.geology.get(wx + dx, wz + dz))
            .filter(|n| n.submerged())
            .map(|n| height - n.height + 4)
            .max()
            .unwrap_or(0);
        // A river's tunnel lies far under the surface: rock under its floor, as under an
        // open river's bed, and past its floor beside it, so no cave opens into its water
        // or breaches the floor where its water stands level with it.
        let tunnel = [(0, 0, 10), (1, 0, 4), (-1, 0, 4), (0, 1, 4), (0, -1, 4)]
            .into_iter()
            .filter_map(|(dx, dz, keep)| self.geology.get(wx + dx, wz + dz).cavity.map(|cavity| height - cavity.floor + keep))
            .max()
            .unwrap_or(0);
        let clearance = Caves::clearance(&geo).max(beside).max(tunnel);
        // Every carver leaves the `clearance` blocks under the surface whole: a river or lake
        // keeps its bed, ravines and sinkholes included. Nothing above reads the lattices.
        let top = height - clearance;

        let p = &mut *self.profiles;
        for (lattice, profile) in self.tunnels.iter().zip(&mut p.tunnels) {
            lattice.profile_to(x, z, top, profile);
        }
        if let Some(cheese) = &self.cheese
            && cavern + giant > 0.0
        {
            cheese.profile_to(x, z, top, &mut p.cheese);
        }
        if let Some(lattice) = &self.giant
            && giant > 0.0
        {
            lattice.profile_to(x, z, top, &mut p.giant);
        }
        if let Some(lattices) = &self.noodles
            && noodle > 0.0
        {
            lattices[0].profile_to(x, z, top, &mut p.noodles[0]);
            lattices[1].profile_to(x, z, top, &mut p.noodles[1]);
        }
        if let Some(lattices) = &self.labyrinth
            && labyrinth > 0.0
        {
            lattices[0].profile_to(x, z, top, &mut p.labyrinth[0]);
            lattices[1].profile_to(x, z, top, &mut p.labyrinth[1]);
        }
        let column = Vec2::new(wx as f32 + 0.5, wz as f32 + 0.5);
        let ravines: Vec<RavineSpan> = self
            .ravines
            .iter()
            .map(|r| {
                let (distance, point) = r.nearest(column);
                RavineSpan { distance, point, ledges: r.ledges }
            })
            .filter(|s| s.distance < s.point.half_width * 1.25)
            .collect();
        let pillar = if cavern + giant > 0.0 { caves.pillars.get_noise_2d(wx as f32, wz as f32) } else { -1.0 };

        let sinkholes: Vec<&Sinkhole> = self.sinkholes.iter().filter(|s| s.centre.distance_squared(column) < (2.0 * s.radius).powi(2)).collect();
        // The hollows of geodes are open like caves.
        for geode in &self.geodes {
            if let Some(span) = geode.span(wx, wz) {
                for y in span.filter(|&y| (FLOOR..=height).contains(&y)) {
                    if geode.distance(IVec3::new(wx, y, wz)) < geode.radius {
                        out[y as usize] = Cave::Air;
                    }
                }
            }
        }
        let tunnel_scale = caves.tunnel_scale(self.width.at(x, z));
        let heights = &caves.heights;
        let noodle_radius = 1.25 * noodle.sqrt() * GRADIENT * NOODLE_FREQUENCY;
        let maze_gradient = GRADIENT * LABYRINTH_FREQUENCY;

        // Tunnels, at every height at once.
        if top >= FLOOR {
            let n = (top - FLOOR + 1) as usize;
            let deep = ((top.min(DEEP_TUNNELS) - FLOOR + 1).max(0) as usize).min(n);
            let rows = &mut p.rows;
            for (row, (profile, len)) in rows.iter_mut().zip(p.tunnels.iter().zip([n, n, deep])) {
                row.resize(len, 0.0);
                profile.fill(FLOOR, row);
            }
            let (ra, rb, rc) = (&rows[0], &rows[1], &rows[2]);
            // Low down, where they close at no surface and the deep families join in.
            let low = deep.min(((height - 10 - FLOOR + 1).max(0)) as usize);
            for i in 0..low {
                let y = FLOOR as usize + i;
                let r = tunnel_scale * heights.growth[y];
                let more = r * heights.more[y];
                let (a, b, c) = (ra[i], rb[i], rc[i]);
                let open = (a * a + b * b < r * r) | ((a * a + c * c).min(b * b + c * c) < more * more);
                out[y] = if open { Cave::Air } else { out[y] };
            }
            for i in low..n {
                let y = FLOOR + i as i32;
                let r = tunnel_scale * heights.growth[y as usize] * tunnel_fade(entrance, height - y);
                let (a, b) = (ra[i], rb[i]);
                let mut open = a * a + b * b < r * r;
                if !open && y <= DEEP_TUNNELS {
                    let r = r * heights.more[y as usize];
                    let c = rc[i];
                    open = (a * a + c * c).min(b * b + c * c) < r * r;
                }
                if open {
                    out[y as usize] = Cave::Air;
                }
            }
        }
        // The rarer carvers, where there are any.
        let others = !ravines.is_empty() || !sinkholes.is_empty() || cavern + giant > 0.0 || noodle > 0.0 || labyrinth > 0.0;
        for y in (FLOOR..=top).filter(|_| others) {
            if out[y as usize] == Cave::Air {
                continue;
            }
            let depth = height - y;
            let mut open = ravines.iter().any(|r| r.contains(y)) || sinkholes.iter().any(|s| s.contains(column, y));
            let deep = heights.deep[y as usize];
            if !open && cavern + giant > 0.0 && depth >= 10 {
                // Caverns grow with depth and stay well clear of the surface.
                let room = smoothstep(10.0, 36.0, depth as f32) * (0.6 + 0.4 * deep);
                let mut margin = p.cheese.at(y) - (0.78 - 0.42 * cavern * room);
                if giant > 0.0 {
                    margin = margin.max(p.giant.at(y) - (0.8 - 0.7 * giant * room));
                }
                // Pillars: wide where they meet floor and ceiling, slender between.
                let inside = (margin / 0.12).clamp(0.0, 1.0);
                open = margin > 0.0 && pillar < 0.6 + 0.22 * inside;
            }
            if !open && noodle > 0.0 && depth >= 5 {
                let (a, b) = (p.noodles[0].at(y), p.noodles[1].at(y));
                open = a * a + b * b < noodle_radius * noodle_radius;
            }
            if !open && labyrinth > 0.0 && depth >= 10 && y >= 16 {
                // Passages about 4 blocks wide and 4 high.
                let a = p.labyrinth[0].at(y) / (0.85 * maze_gradient * 2.0);
                let b = p.labyrinth[1].at(y) / LABYRINTH_PASSAGE;
                open = a * a + b * b < labyrinth;
            }
            if open {
                out[y as usize] = Cave::Air;
            }
        }
        // On flat ground a tunnel grazing the surface would scrape a shallow trough; there
        // the ground only opens where the hole leads at least `MOUTH` blocks down. Hillsides
        // keep their sideways mouths.
        if self.geology.slope(wx, wz) <= 1 && height - MOUTH >= FLOOR {
            let mut deep = out[(height - MOUTH) as usize] != Cave::Rock;
            for y in height - MOUTH + 1..=height {
                deep &= out[y as usize] != Cave::Rock;
                if !deep {
                    out[y as usize] = Cave::Rock;
                }
            }
        }
        self.fill(x, z, &geo, out);
    }

    /// Whether caves open the ground at (x, z) (world coordinates, within reach of features
    /// around the column) to the sky, so nothing should grow there. It reads noise directly
    /// rather than from the column's lattices, so every column asking about the same spot
    /// gets the same answer; it can differ from `carve` by a block at the edge of a mouth.
    pub fn surface_open(&self, x: i32, z: i32, geo: &GeoSample) -> bool {
        let caves = self.caves;
        let y = geo.height;
        let column = Vec2::new(x as f32 + 0.5, z as f32 + 0.5);
        let ravine = self.ravines.iter().any(|r| {
            let (distance, point) = r.nearest(column);
            RavineSpan { distance, point, ledges: r.ledges }.contains(y)
        });
        if ravine || self.sinkholes.iter().any(|s| s.contains(column, y)) {
            return true;
        }
        let (xf, zf) = (x as f32, z as f32);
        let entrance = smoothstep(caves.entrance_cut, caves.entrance_cut + 0.1, caves.entrances.get_noise_2d(xf, zf));
        if entrance <= 0.0 || Caves::clearance(geo) > 0 {
            return false;
        }
        let r = caves.tunnel_radius(caves.tunnel_width.get_noise_2d(xf, zf), entrance, y, 0);
        let [a, b] = [0, 1].map(|i| caves.tunnels[i].get_noise_3d(xf, y as f32 * TUNNEL_SCALE_Y, zf));
        a * a + b * b < r * r
    }

    /// Fills carved blocks with water and lava from the water tables, and seals them where
    /// they would leave a fluid beside air, or sea water above air. Geodes stay dry: the
    /// water tables' barriers would otherwise wall rock through their hollows, so instead
    /// flooded cave beside a geode's hollow turns to rock (which the lining makes crystal).
    fn fill(&self, x: i32, z: i32, geo: &GeoSample, out: &mut [Cave]) {
        let (wx, wz) = (self.origin.x + x, self.origin.z + z);
        let height = geo.height;
        let offset = Vec3::new(self.warp[0].at(x, z) * WARP_XZ, self.warp[1].at(x, z) * WARP_Y, self.warp[2].at(x, z) * WARP_XZ);
        // Open air or sea beside or above the ground: below the lowest neighbouring surface,
        // every side is ground.
        let lowest = [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|(dx, dz)| self.geology.get(wx + dx, wz + dz).height).into_iter().min().unwrap_or(height);
        let outside = |y: i32| if y <= SEA_LEVEL { Cave::Water } else { Cave::Air };
        // Geodes reaching this block column or the ones beside it.
        let geodes: Vec<&Geode> = self.geodes.iter().filter(|g| (-1..=1).any(|dz| (-1..=1).any(|dx| g.span(wx + dx, wz + dz).is_some()))).collect();
        let hollow = |p: IVec3| geodes.iter().any(|g| g.distance(p) < g.radius);
        for y in FLOOR..=height {
            let cave = &mut out[y as usize];
            if *cave == Cave::Rock {
                continue;
            }
            let pos = IVec3::new(wx, y, wz);
            if !geodes.is_empty() && hollow(pos) {
                *cave = Cave::Air;
                continue;
            }
            *cave = self.tables.fluid(Vec3::new(wx as f32, y as f32, wz as f32) + offset, y);
            if *cave != Cave::Air && !geodes.is_empty() && [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z].iter().any(|&d| hollow(pos + d)) {
                *cave = Cave::Rock;
                continue;
            }
            let exposed = y > lowest && *cave != outside(y);
            let under_sea = y == height && *cave == Cave::Air && outside(y + 1) == Cave::Water;
            if exposed || under_sea {
                *cave = Cave::Rock;
            }
        }
    }
}

impl Sinkhole {
    #[inline]
    fn contains(&self, column: Vec2, y: i32) -> bool {
        if y < self.bottom || y > self.surface + 12 {
            return false;
        }
        let offset = column - self.centre;
        let reach = self.radius * 2.0;
        if offset.length_squared() > reach * reach {
            return false;
        }
        let angle = offset.y.atan2(offset.x);
        let wobble = 1.0 + 0.15 * (3.0 * angle + self.phases[0]).sin() + 0.1 * (5.0 * angle + self.phases[1]).sin();
        // A funnel at the rim and a rounded bottom.
        let (y, bottom, surface) = (y as f32, self.bottom as f32, self.surface as f32);
        let rim = 1.0 + 0.6 * smoothstep(surface - 8.0, surface + 1.0, y);
        let base = (1.0 - ((bottom + 6.0 - y) / 6.0).max(0.0).powi(2)).max(0.0).sqrt();
        offset.length() < self.radius * wobble * rim * base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_vary_but_stay_in_range() {
        let styles: Vec<CaveStyle> = (0..100).map(|seed| CaveStyle::sample(seed, &WorldDials::sample(seed))).collect();
        assert_eq!(styles[5], CaveStyle::sample(5, &WorldDials::sample(5)));
        for s in &styles {
            for v in [s.tunnels, s.caverns, s.giants, s.noodles, s.labyrinths, s.ravines, s.sinkholes, s.flooding] {
                assert!((0.05..=10.0).contains(&v), "{s:?}");
            }
        }
        let most = styles.iter().map(|s| s.ravines).fold(0.0, f32::max);
        let least = styles.iter().map(|s| s.ravines).fold(f32::MAX, f32::min);
        assert!(most > 4.0 * least, "ravines vary only {least}..{most}");
    }

    #[test]
    fn states_follow_levels() {
        assert_eq!(Tables::state(DRY, 5), Cave::Lava);
        assert_eq!(Tables::state(DRY, LAVA_LEVEL), Cave::Air);
        assert_eq!(Tables::state(40, 5), Cave::Water);
        assert_eq!(Tables::state(40, 39), Cave::Water);
        assert_eq!(Tables::state(40, 40), Cave::Air);
    }
}

impl Caves {
    /// Whether a sinkhole or a ravine may open the ground within `radius` blocks of
    /// `centre` (x, z): settlements keep away from them.
    pub(crate) fn opens_near(&self, centre: (i32, i32), radius: i32) -> bool {
        // As `sinkholes_near` places them, the widest 16 blocks across.
        const REACH: i32 = 32 + features::REACH;
        let here = Vec2::new(centre.0 as f32, centre.1 as f32);
        let cell = |v: i32| v.div_euclid(SINKHOLE_CELL);
        for sz in cell(centre.1 - radius - 16)..=cell(centre.1 + radius + 16) {
            for sx in cell(centre.0 - radius - 16)..=cell(centre.0 + radius + 16) {
                let mut rng = Rng::new(self.seed, SINKHOLES, sx, sz);
                if rng.chance(0.035 * self.style.sinkholes) {
                    let span = (SINKHOLE_CELL - 2 * REACH) as u32;
                    let at = Vec2::new((sx * SINKHOLE_CELL + REACH + rng.below(span)) as f32, (sz * SINKHOLE_CELL + REACH + rng.below(span)) as f32);
                    if at.distance(here) < (radius + 16) as f32 {
                        return true;
                    }
                }
            }
        }
        let cell = |v: i32| v.div_euclid(RAVINE_CELL);
        for rz in cell(centre.1 - 1300 - radius)..=cell(centre.1 + 1300 + radius) {
            for rx in cell(centre.0 - 1300 - radius)..=cell(centre.0 + 1300 + radius) {
                if let Some(path) = self.ravine_path(rx, rz)
                    && path.points.iter().any(|p| p.distance(here) < radius as f32 + path.width)
                {
                    return true;
                }
            }
        }
        false
    }
}

impl Caves {
    /// Places near `centre` (within `radius`) worth looking at, for previews and captures:
    /// the strongest spot of each kind of region, and the nearest ravine point.
    #[cfg(test)]
    pub fn sights(&self, centre: (i32, i32), radius: i32) -> Vec<(&'static str, i32, i32)> {
        let mut best = [("caverns", f32::MIN, 0, 0), ("giant caverns", f32::MIN, 0, 0), ("labyrinth", f32::MIN, 0, 0), ("noodles", f32::MIN, 0, 0), ("entrances", f32::MIN, 0, 0)];
        for dz in (-radius..=radius).step_by(16) {
            for dx in (-radius..=radius).step_by(16) {
                let (x, z) = (centre.0 + dx, centre.1 + dz);
                let (xf, zf) = (x as f32, z as f32);
                let values = [
                    self.cavern_regions.get_noise_2d(xf, zf) - self.cavern_cut,
                    self.giant_regions.get_noise_2d(xf, zf) - self.giant_cut,
                    self.labyrinth_regions.get_noise_2d(xf, zf) - self.labyrinth_cut,
                    self.noodle_regions.get_noise_2d(xf, zf) - self.noodle_cut,
                    self.entrances.get_noise_2d(xf, zf) - self.entrance_cut,
                ];
                for (b, v) in best.iter_mut().zip(values) {
                    if v > b.1 {
                        *b = (b.0, v, x, z);
                    }
                }
            }
        }
        let mut sights: Vec<_> = best.into_iter().filter(|b| b.1 > 0.05).map(|b| (b.0, b.2, b.3)).collect();
        let cell = |v: i32| v.div_euclid(RAVINE_CELL);
        let here = Vec2::new(centre.0 as f32, centre.1 as f32);
        let mut nearest: Option<(f32, Vec2)> = None;
        for rz in cell(centre.1 - 1300 - radius)..=cell(centre.1 + 1300 + radius) {
            for rx in cell(centre.0 - 1300 - radius)..=cell(centre.0 + 1300 + radius) {
                // Away from the narrow ends.
                let points = self.ravine_path(rx, rz).map(|p| p.points).unwrap_or_default();
                let n = points.len();
                for p in points.into_iter().skip(n / 3).take(n / 3) {
                    if nearest.is_none_or(|n| p.distance(here) < n.0) {
                        nearest = Some((p.distance(here), p));
                    }
                }
            }
        }
        if let Some((d, p)) = nearest
            && d < radius as f32
        {
            sights.push(("ravine", p.x as i32, p.y as i32));
        }
        for sz in (centre.1 - radius).div_euclid(SINKHOLE_CELL)..=(centre.1 + radius).div_euclid(SINKHOLE_CELL) {
            for sx in (centre.0 - radius).div_euclid(SINKHOLE_CELL)..=(centre.0 + radius).div_euclid(SINKHOLE_CELL) {
                let mut rng = Rng::new(self.seed, SINKHOLES, sx, sz);
                if rng.chance(0.035 * self.style.sinkholes) {
                    // As `sinkholes_near` places it.
                    let margin = 32 + features::REACH;
                    let span = (SINKHOLE_CELL - 2 * margin) as u32;
                    let x = sx * SINKHOLE_CELL + margin + rng.below(span);
                    sights.push(("sinkhole", x, sz * SINKHOLE_CELL + margin + rng.below(span)));
                }
            }
        }
        sights
    }
}

/// Finding places to look at caves (for previews and in-game captures).
#[cfg(test)]
mod locate {
    use super::*;

    /// `cargo test --release -p voxmar_worldgen locate_caves -- --ignored --nocapture`
    /// (optionally `SEED=n`) lists the nearest ravines, giant caverns, labyrinths and
    /// sinkholes to the origin.
    #[test]
    #[ignore]
    fn locate_caves() {
        let seed = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0x5eed);
        let stream = crate::rng::stream_seed(seed, crate::Overworld::ID, crate::Overworld::VERSION);
        let caves = Caves::new(stream, &WorldDials::sample(stream));
        println!("{:#?}", caves.style);
        for rz in -4..4 {
            for rx in -4..4 {
                if let Some(path) = caves.ravine_path(rx, rz) {
                    let (a, b) = (path.points[0], path.points[path.points.len() / 2]);
                    println!("ravine from {a} via {b}, {} long, {:.0} deep, {:.0} wide, {:.0} under", path.points.len() as f32 * RAVINE_STEP, path.depth, path.width, path.cover);
                }
            }
        }
        let mut found = [0; 3];
        for i in 0..160 * 160 {
            let (x, z) = ((i % 160 - 80) * 64, (i / 160 - 80) * 64);
            let (xf, zf) = (x as f32, z as f32);
            if found[0] < 6 && caves.giant_regions.get_noise_2d(xf, zf) > caves.giant_cut + 0.1 {
                found[0] += 1;
                println!("giant caverns at {x} {z}");
            }
            if found[1] < 6 && caves.labyrinth_regions.get_noise_2d(xf, zf) > caves.labyrinth_cut + 0.1 {
                found[1] += 1;
                println!("labyrinth at {x} {z}");
            }
            if found[2] < 6 && caves.cavern_regions.get_noise_2d(xf, zf) > caves.cavern_cut + 0.2 {
                found[2] += 1;
                println!("caverns at {x} {z}");
            }
        }
    }
}
