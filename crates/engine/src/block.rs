//! Block definitions and the registry that compiles them into lookup tables.
//!
//! Definitions are plain data (RON). First-party content goes through exactly the same
//! loading path a mod's content will.
//!
//! Three things keep content files short as they grow:
//!
//! - **Families.** A template is a list of block definitions with `{name}` in their
//!   names, textures and drops (a wood: log, stripped log, wood, planks, leaves, sapling).
//!   A family is one line that makes the whole set for one name (`maple`), with its own
//!   tags and changes to single members. Templates are shared across packages, so a mod's
//!   new wood uses `base:wood`.
//! - **Variants.** A plant (or any block) can list other looks of itself: more blooms, a
//!   different leaf shape. Each becomes its own block, named `<block>_<variant>`, that
//!   drops the plant itself, as many as the variant's size. Generators pick a look per
//!   position with [`BlockRegistry::looks`].
//! - **Tags.** Words other systems select blocks by, rather than naming them: what a block
//!   is (`stone`, `flower`), where it fits (`wetland`), what it is good for (`edible`,
//!   `dye:red`). The vocabulary is listed in `assets/content/base/blocks.ron`.
//!
//! A block with a `fluid` (water, lava) also makes the blocks of its other states, right
//! after it: flowing, falling and, for rivers, pinned water (see [`crate::fluid`]).

use std::collections::BTreeMap;

use glam::IVec3;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::coords::Face;
use crate::fluid::{self, Fluid, FluidDef, FluidId, FluidState};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BlockId(pub u16);

impl BlockId {
    pub const AIR: BlockId = BlockId(0);

    pub fn is_air(self) -> bool {
        self == Self::AIR
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderKind {
    /// Not drawn at all.
    Invisible,
    /// Fully opaque cube. Hides neighbouring faces and blocks light.
    #[default]
    Opaque,
    /// Alpha-tested cube (glass). Does not hide neighbours; faces between two of the same
    /// block are hidden.
    Cutout,
    /// Alpha-tested cube that draws every face, also between two of its kind, so its holes
    /// show depth from every angle (Minecraft's fancy leaves).
    Leaves,
    /// Alpha-blended cube (water, ice). Faces between two of the same block are hidden.
    Translucent,
    /// Two crossed, alpha-tested quads through the cell's diagonals (grass, flowers). Hides
    /// nothing and lets light through. A plant gives way to a block placed into it and
    /// goes when the block beneath it does.
    Plant,
    /// Drawn as its shape's boxes (`shape`, see [`crate::shape`]): a barrel, a crate, a slab,
    /// stairs, a door. Alpha-tested. It hides its neighbours' faces and blocks light only
    /// where its boxes cover a whole side of the cell.
    Model,
}

/// The material a block sounds like when stepped on, placed or broken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Sound {
    #[default]
    Stone,
    Wood,
    Soil,
    Grass,
    Sand,
    Gravel,
    Snow,
    Glass,
    Leaves,
    Water,
    /// Wet, squelching ground: mud, peat.
    Mud,
    /// Soft, springy ground cover.
    Moss,
}

impl Sound {
    pub const ALL: [Sound; 12] = [
        Sound::Stone,
        Sound::Wood,
        Sound::Soil,
        Sound::Grass,
        Sound::Sand,
        Sound::Gravel,
        Sound::Snow,
        Sound::Glass,
        Sound::Leaves,
        Sound::Water,
        Sound::Mud,
        Sound::Moss,
    ];
}

/// Which biome colour multiplies a face's texture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tint {
    #[default]
    None,
    Grass,
    Foliage,
    Water,
}

/// What a block definition says about tint: a biome colour the renderer multiplies in (a
/// [`Tint`]), or one fixed colour baked into the block's textures when the texture array is
/// built (`TextureRef::tint`). A greyscale drawing then makes any number of colour variants
/// (dyed cloth, stained wood) from data alone, and the renderer sees an untinted block: no
/// vertex bit, and no pass of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockTint {
    #[default]
    None,
    Grass,
    Foliage,
    Water,
    /// This colour: red, green and blue.
    Fixed(u8, u8, u8),
}

impl BlockTint {
    /// The colour baked into the textures, for [`BlockTint::Fixed`].
    pub fn baked(self) -> Option<[u8; 3]> {
        match self {
            BlockTint::Fixed(r, g, b) => Some([r, g, b]),
            _ => None,
        }
    }

    /// The tint the renderer draws the block's faces with: a biome colour, or none for a
    /// baked one.
    pub fn drawn(self) -> Tint {
        match self {
            BlockTint::None | BlockTint::Fixed(..) => Tint::None,
            BlockTint::Grass => Tint::Grass,
            BlockTint::Foliage => Tint::Foliage,
            BlockTint::Water => Tint::Water,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Textures {
    None,
    All(String),
    /// Logs and similar: `end` on top and bottom, `side` around.
    Column { end: String, side: String },
    Sides { top: String, bottom: String, side: String },
}

impl Textures {
    fn names_mut(&mut self) -> Vec<&mut String> {
        match self {
            Textures::None => Vec::new(),
            Textures::All(t) => vec![t],
            Textures::Column { end, side } => vec![end, side],
            Textures::Sides { top, bottom, side } => vec![top, bottom, side],
        }
    }

    /// The texture of `face`, if it has one.
    pub fn face(&self, face: Face) -> Option<&str> {
        Some(match (self, face) {
            (Textures::None, _) => return None,
            (Textures::All(t), _) => t,
            (Textures::Column { end, .. }, Face::PosY | Face::NegY) => end,
            (Textures::Column { side, .. }, _) => side,
            (Textures::Sides { top, .. }, Face::PosY) => top,
            (Textures::Sides { bottom, .. }, Face::NegY) => bottom,
            (Textures::Sides { side, .. }, _) => side,
        })
    }
}

/// The least and most a fire's ward reaches (`BlockDef::ward`, `MultiblockDef::ward`).
pub const WARD_LEAST: f32 = 2.0;
pub const WARD_MOST: f32 = 24.0;

/// A block definition as authored in content files.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BlockDef {
    pub name: String,
    pub render: RenderKind,
    /// Whether entities collide with the block.
    pub solid: bool,
    pub textures: Textures,
    pub tint: BlockTint,
    /// Only tint the top face (grass blocks).
    pub tint_top_only: bool,
    /// Light level emitted, 0..=15.
    pub light: u8,
    /// Extra light attenuation for non-opaque blocks (water, leaves).
    pub light_filter: u8,
    /// Seconds to break by hand. Negative means unbreakable.
    pub hardness: f32,
    pub sound: Sound,
    /// The building material it sounds as (a `materials` entry of sounds.ron, by name),
    /// whatever its tags say: cut cobblestone is not `cobbled` for recipes but still sounds
    /// like cobblestone. Only sounds read it; nothing else may.
    pub material: Option<String>,
    /// Name of the block dropped when broken, `core:air` for nothing. Defaults to the
    /// block itself.
    pub drops: Option<String>,
    /// How many of `drops` breaking it gives.
    pub drop_count: u8,
    /// Placeholder texture colour used when no texture pack is available.
    pub color: [u8; 3],
    /// Words other systems select blocks by (see the module docs).
    pub tags: Vec<String>,
    /// The family the block belongs to (`base:maple`), set for blocks made from a template.
    pub family: Option<String>,
    /// Inventory icon: an item texture (`item/<icon>` in the texture packs). Without one,
    /// the icon is drawn from the block's own textures: a plant's sprite or a cube.
    pub icon: Option<String>,
    /// How rare the block is meant to be where it occurs naturally. Generators scale how
    /// often they place it by this unless their own data says otherwise.
    pub rarity: Rarity,
    /// Other looks of this block, each made into its own block when content loads.
    pub variants: Vec<Variant>,
    /// For a block made from another's variant: that block's name.
    pub variant_of: Option<String>,
    /// Makes the block the source of a fluid that flows (see [`crate::fluid`]).
    pub fluid: Option<FluidDef>,
    /// What the block needs to stay: plants need ground (`Below`, their default), hanging
    /// plants and stalactites a ceiling (`Above`), other blocks nothing.
    pub support: Option<Support>,
    /// Only ever an item, never placed in the world: seeds, fruit, produce. It is drawn as
    /// a sprite of its texture in the hand and on the ground.
    pub item: bool,
    /// The action using it performs instead of placing it (`base:plant` for a seed), by
    /// its full name.
    pub action: Option<String>,
    /// How its textures vary from block to block, so a large motif does not repeat in a
    /// grid across a wall or a floor. Opaque blocks only.
    pub texture_variety: TextureVariety,
    /// Its bright texels glow by the block light passing through it, as a window of a lit
    /// room does at night (a pane of oiled hide, glass). Not by its own light.
    pub lit_through: bool,
    /// The shape a plant is drawn as (`render: Plant`).
    pub model: PlantModel,
    /// How tall a plant stands, as a share of its usual height (each plant also varies a
    /// little by itself): 1.4 for a big flower. Up to about 3.9.
    pub plant_height: f32,
    /// The block it is drawn over, by name: an ore over the rock it lies in. Each face's
    /// texture is drawn over the host's, from whichever pack draws the host, so the rock
    /// around the ore always matches the rock around it in the world; its own textures
    /// need only the ore, on clear texels. Any ore can then lie in any rock.
    pub host: Option<String>,
    /// Particles it gives off while near the player: a campfire's smoke, embers and sparks,
    /// a torch's flame. Only the client draws them.
    pub particles: Vec<Emission>,
    /// How many blocks round it its firelight keeps the wild back at night: creatures afraid
    /// of fire (`creature::Fire`) do not come in. 0 for none, else 2 to 24.
    pub ward: f32,
    /// Whether it comes in four facings: the block itself faces north (-z), and blocks
    /// `<name>_east`, `<name>_south` and `<name>_west` are made from it, turned (their
    /// particles too), each dropping the block itself. A wall torch faces out from its
    /// wall; a stair, door or bed the way it was placed.
    pub facing: bool,
    /// For a block made in one of its facings (see `facing`): which way it faces.
    pub faces: Option<Facing>,
    /// Whether it also comes upside down (a block drawn as a shape): `<name>_top` is made
    /// from it, its shape turned over (a top slab, stairs under a ceiling), and made in its
    /// facings too if it has them, each dropping the block itself. Placed against the
    /// underside of a block, it goes upside down.
    pub flips: bool,
    /// For a block made upside down from another (see `flips`).
    pub flipped: bool,
    /// Its shape, by name (`base:barrel`), for a block drawn as one (`render: Model`).
    pub shape: Option<String>,
    /// The block placed instead when this one is placed against a block's side, by name,
    /// facing out from it: a torch on a wall is a wall torch (`facing` and `support: Wall`).
    pub wall: Option<String>,
    /// The block it turns into when used (opened or shut), by name, in the same facing: a
    /// shut door's open form, and the open form's shut one. Its upper half (see `upper`)
    /// turns with it. The server turns it first, so its collision changes before anyone's.
    pub swaps: Option<String>,
    /// The block placed in the cell above along with it, by name, in the same facing, and
    /// broken with it: a door's top half, which rests on it (`support: Below`).
    pub upper: Option<String>,
    /// What coats it in place (`with`, a block held, one used up) and what it is then
    /// (`into`): clay on a wattle panel daubs it. It takes the level of the recipe that makes
    /// `into` and teaches its experience a piece (server `coat.rs`).
    pub coat: Option<Coat>,
}

/// A coating (see [`BlockDef::coat`]): the block held to coat it, and the block it becomes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coat {
    pub with: String,
    pub into: String,
}

/// The name suffix of a block made upside down (see [`BlockDef::flips`]), before any facing's.
const FLIPPED: &str = "_top";

/// One of the four ways a block can face (see [`BlockDef::facing`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Facing {
    /// Towards -z.
    #[default]
    North,
    /// Towards +x.
    East,
    /// Towards +z.
    South,
    /// Towards -x.
    West,
}

impl Facing {
    pub const ALL: [Facing; 4] = [Facing::North, Facing::East, Facing::South, Facing::West];

    pub const fn normal(self) -> IVec3 {
        match self {
            Facing::North => IVec3::NEG_Z,
            Facing::East => IVec3::X,
            Facing::South => IVec3::Z,
            Facing::West => IVec3::NEG_X,
        }
    }

    /// The facing pointing out of a block's side; none for its top and bottom.
    pub fn of_face(face: Face) -> Option<Facing> {
        Facing::ALL.into_iter().find(|f| f.normal() == face.normal())
    }

    /// The name suffix of the block made in this facing (the block itself faces north).
    fn suffix(self) -> &'static str {
        match self {
            Facing::North => "",
            Facing::East => "_east",
            Facing::South => "_south",
            Facing::West => "_west",
        }
    }

    /// A point in a cell (from its lower corner, in blocks) given for the block facing
    /// north, where it lies once the block is turned to face this way.
    pub fn turn(self, at: [f32; 3]) -> [f32; 3] {
        let (x, z) = (at[0] - 0.5, at[2] - 0.5);
        // Clockwise seen from above: north (-z) to east (+x) to south (+z) to west.
        let (x, z) = match self {
            Facing::North => (x, z),
            Facing::East => (-z, x),
            Facing::South => (-x, -z),
            Facing::West => (z, -x),
        };
        [x + 0.5, at[1], z + 0.5]
    }
}

/// A stream of particles a block gives off, as the client draws it near the player. The
/// `style` decides how they look, move and how long they last; the rest says where they
/// start, how often, and lets a mod recolour or resize them (a green alchemist's fire).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Emission {
    pub style: ParticleStyle,
    /// Times a second it gives off particles, on average: they come at random moments.
    pub rate: f32,
    /// Particles each time: more than one makes a burst (a fire's crackle of sparks).
    pub count: u8,
    /// Where they start, from the cell's lower corner, in blocks.
    pub at: [f32; 3],
    /// How far across from `at` they may start, in blocks.
    pub spread: f32,
    /// sRGB colour, instead of the style's own.
    pub color: Option<[u8; 3]>,
    /// Scales the style's size.
    pub size: f32,
    /// About how high they rise, in blocks, instead of the style's own: a campfire's
    /// flames lick a block up, a candle's a hand's breadth. They rise faster and last
    /// longer to get there.
    pub height: Option<f32>,
    /// How they are drawn over what is behind them, instead of the style's own: flames,
    /// embers and sparks add their light (`Add`), smoke is a soft veil (`Veil`), and a
    /// `Cutout` particle is solid where it shows, as a piece of a broken block is.
    pub blend: Option<ParticleBlend>,
    /// How much of what lies behind a particle that adds light it hides, 0..1, instead of
    /// the style's own: a spark is solid and hides it all; a flame's tongue hides some.
    pub hides: Option<f32>,
    /// How far off it is seen, in blocks, instead of the client's own nearness (24), up to
    /// [`MAX_EMISSION_RANGE`]: a chimney's smoke shows from across a valley.
    pub range: Option<f32>,
    /// `(tag, reach)`: given off only while a block with `tag` lies below it, up to `reach`
    /// blocks down and one across, and only from the top of a stack (open air above it): a
    /// chimney smokes over a hearth, from its top.
    pub over: Option<(String, u8)>,
}

/// The farthest a block's particles are seen (`Emission::range`), in blocks.
pub const MAX_EMISSION_RANGE: f32 = 128.0;
/// The deepest a block looks for what it smokes over (`Emission::over`), in blocks.
pub const MAX_EMISSION_OVER: u8 = 8;

/// Tags of a window, a block light comes in through (a pane, a lattice, a window frame, an
/// arrow slit, or a wall a draught comes through), refused at load if it keeps light out.
pub const LIGHT_THROUGH_TAGS: [&str; 5] = ["window", "lattice", "window_frame", "slit", "draught"];
/// The tag of a railing, refused at load unless it is solid and a block high.
pub const RAILING_TAG: &str = "railing";
/// The tag of a chimney piece (a flue, a hood, a pot): stacked straight over a block that
/// smokes, the client brings its smoke out of the top of the stack, and a piece lines a
/// hearth's flue against its fire as stone does, so it is refused at load unless solid.
pub const CHIMNEY_PIECE_TAG: &str = "chimney_piece";
/// The tag of a ladder (`BlockInfo::climbable`): a player whose feet are in its cell climbs
/// it, pressing against it or holding jump (`movement::step`), and is refused at load unless
/// solid, so there is something to press against.
pub const CLIMBABLE_TAG: &str = "climbable";
/// The tag of a landing, refused at load unless it is solid across the whole top of its
/// cell (level with a stair's top).
pub const LANDING_TAG: &str = "landing";
/// The most a window may dim the light through it (`light_filter`): a shut shutter's 14
/// is a wall's.
pub const MAX_WINDOW_FILTER: u8 = 2;

/// How a particle is drawn over what lies behind it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParticleBlend {
    /// Solid where it shows, cut out elsewhere, written to depth like a block.
    #[default]
    Cutout,
    /// Adds its light to what is behind it: fire, embers, sparks, glowing motes. Overlapping
    /// flames merge into one brighter flame, and nothing has to be sorted.
    Add,
    /// A soft translucent veil, thinnest at its edges and where it meets anything solid:
    /// smoke, steam, dust.
    Veil,
}

impl ParticleStyle {
    /// How much of what lies behind it a particle of the style hides when it adds light,
    /// unless an emission says otherwise (`Emission::hides`): embers and sparks are solid
    /// specks, so over a bright day sky they stay their colour instead of a salmon tint.
    pub fn hides(self) -> f32 {
        match self {
            ParticleStyle::Flame => 0.6,
            ParticleStyle::Ember | ParticleStyle::Spark => 1.0,
            ParticleStyle::Smoke => 0.0,
        }
    }

    /// How the style is drawn unless an emission says otherwise.
    pub fn blend(self) -> ParticleBlend {
        match self {
            ParticleStyle::Smoke => ParticleBlend::Veil,
            ParticleStyle::Ember | ParticleStyle::Spark | ParticleStyle::Flame => ParticleBlend::Add,
        }
    }
}

impl Default for Emission {
    fn default() -> Self {
        Self {
            style: ParticleStyle::Smoke,
            rate: 1.0,
            count: 1,
            at: [0.5, 0.5, 0.5],
            spread: 0.1,
            color: None,
            size: 1.0,
            height: None,
            blend: None,
            hides: None,
            range: None,
            over: None,
        }
    }
}

/// How particles a block gives off look and move.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParticleStyle {
    /// Soft grey puffs that rise, slow, swell and drift on the wind.
    #[default]
    Smoke,
    /// Glowing motes that float up a block or two and cool to nothing.
    Ember,
    /// Bright pinpricks thrown up that arc over and die on the ground.
    Spark,
    /// Tiny tongues of flame licking up off a wick, shrinking away as they flicker.
    Flame,
}

/// The shape a plant is drawn as, all of quads of its texture that sway with the wind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlantModel {
    /// Two quads crossing on the cell's diagonals: grass, ferns and tall grasses.
    #[default]
    Cross,
    /// Four quads in a #, straight with the blocks: crops in rows, as Minecraft's wheat.
    Hash,
    /// A mushroom with volume: a stem of the side texture, a domed cap of the top texture
    /// (a crown and four sloping sides) and gills of the bottom texture under it.
    Cap,
    /// Four quads splayed from one root like a bouquet, each facing out from the middle
    /// with its top leaning out: bushes, flowers, wild grains and reeds. No quad runs through
    /// the middle, so none is seen flat on.
    Bush,
    /// A small rock with volume, lying still on the ground: a lump with a flat top and
    /// four sloping sides, and a chip beside it, all of the block's texture, its size
    /// `plant_height` times the usual. Samples of ore over a vein.
    Rock,
    /// Two crossed quads leaning out from the wall behind it, still, as a torch fixed to
    /// a wall: for a block in four facings (see [`BlockDef::facing`]) with `support: Wall`.
    Wall,
}

/// How an opaque block's textures vary from block to block, as Minecraft's stone, sand and
/// dirt do: each block picks, by its position, a quarter turn of its top and bottom, and
/// whether its faces are mirrored left to right. Turning suits textures without an up
/// (stone, sand, gravel, crystal); mirroring nearly anything without lettering.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextureVariety {
    /// Top and bottom faces turn a quarter turn at a time, at random.
    pub turn: bool,
    /// Faces are mirrored left to right at random.
    pub mirror: bool,
}

/// Which neighbour a block rests on. It cannot be placed without a solid block there, and
/// goes when that block does, together with the column of blocks resting on it in turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Support {
    /// Stands on its own.
    #[default]
    None,
    /// Grows up from the block beneath (plants).
    Below,
    /// Hangs from the block above (vines, hanging roots, stalactites).
    Above,
    /// Fixed to the side of the block behind it, the way it does not face: a wall torch.
    Wall,
}

impl Default for BlockDef {
    fn default() -> Self {
        Self {
            name: String::new(),
            render: RenderKind::Opaque,
            solid: true,
            textures: Textures::None,
            tint: BlockTint::None,
            tint_top_only: false,
            light: 0,
            light_filter: 0,
            hardness: 1.0,
            sound: Sound::Stone,
            material: None,
            drops: None,
            drop_count: 1,
            color: [255, 0, 255],
            tags: Vec::new(),
            family: None,
            icon: None,
            rarity: Rarity::Common,
            variants: Vec::new(),
            variant_of: None,
            fluid: None,
            support: None,
            item: false,
            action: None,
            texture_variety: TextureVariety::default(),
            lit_through: false,
            model: PlantModel::Cross,
            plant_height: 1.0,
            host: None,
            particles: Vec::new(),
            ward: 0.0,
            facing: false,
            faces: None,
            flips: false,
            flipped: false,
            wall: None,
            shape: None,
            swaps: None,
            upper: None,
            coat: None,
        }
    }
}

impl BlockDef {
    /// For a block made in one of its facings other than north: the block it was made
    /// from, by name (`base:wall_torch` for `base:wall_torch_east`).
    pub fn facing_of(&self) -> Option<&str> {
        self.faces.and_then(|facing| self.name.strip_suffix(facing.suffix())).filter(|base| *base != self.name)
    }

    /// For a block made from another as one of the forms it is placed in (another facing,
    /// upside down; see `facing` and `flips`): the block it was made from, which it drops
    /// and is named by.
    pub fn form_of(&self) -> Option<&str> {
        let turned = self.facing_of().unwrap_or(&self.name);
        let base = if self.flipped { turned.strip_suffix(FLIPPED).unwrap_or(turned) } else { turned };
        (base != self.name).then_some(base)
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// Replaces `{name}` in every name the definition holds.
    fn substitute(&mut self, name: &str) {
        let fill = |s: &mut String| *s = s.replace("{name}", name);
        fill(&mut self.name);
        self.textures.names_mut().into_iter().for_each(fill);
        self.drops.iter_mut().for_each(fill);
        self.wall.iter_mut().for_each(fill);
        self.swaps.iter_mut().for_each(fill);
        self.upper.iter_mut().for_each(fill);
        if let Some(coat) = &mut self.coat {
            fill(&mut coat.with);
            fill(&mut coat.into);
        }
        self.icon.iter_mut().for_each(fill);
        self.tags.iter_mut().for_each(fill);
        for variant in &mut self.variants {
            fill(&mut variant.name);
            variant.textures.names_mut().into_iter().for_each(fill);
        }
    }

    /// Puts unqualified block names (`stone`) into `namespace` (`base:stone`).
    fn qualify(&mut self, namespace: &str) {
        let qualify = |name: &mut String| {
            if !name.contains(':') {
                *name = format!("{namespace}:{name}");
            }
        };
        qualify(&mut self.name);
        self.drops.iter_mut().for_each(qualify);
        self.host.iter_mut().for_each(qualify);
        self.family.iter_mut().for_each(qualify);
        self.variant_of.iter_mut().for_each(qualify);
        self.wall.iter_mut().for_each(qualify);
        self.swaps.iter_mut().for_each(qualify);
        self.upper.iter_mut().for_each(qualify);
        self.shape.iter_mut().for_each(qualify);
        if let Some(coat) = &mut self.coat {
            qualify(&mut coat.with);
            qualify(&mut coat.into);
        }
        if let Some(fluid) = &mut self.fluid {
            for meeting in &mut fluid.meets {
                for name in [&mut meeting.fluid, &mut meeting.source, &mut meeting.flowing, &mut meeting.below] {
                    qualify(name);
                }
            }
            fluid.crust.iter_mut().for_each(|crust| qualify(&mut crust.block));
        }
    }
}

/// How rare something is, in tiers each several times rarer than the last. Generators
/// decide what each tier means in numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Rarity {
    #[default]
    Common,
    Uncommon,
    Rare,
    VeryRare,
    Legendary,
}

/// Another look of a block: the same plant with more blooms, a different shape or size.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Variant {
    /// Added to the block's name: `poppy` and `pair` make `poppy_pair`.
    pub name: String,
    pub textures: Textures,
    /// How big it is, which is how many of the block it drops: a single bloom 1, a
    /// clump of them 2 or 3.
    pub size: u8,
    /// How often this look appears among the block's looks (the block's own look is
    /// common).
    pub rarity: Rarity,
    /// Its plant model, when it differs from the block's: a clump of mushrooms drawn as a
    /// cross while a single one has a cap.
    pub model: Option<PlantModel>,
}

impl Default for Variant {
    fn default() -> Self {
        Self { name: String::new(), textures: Textures::None, size: 1, rarity: Rarity::Common, model: None }
    }
}

/// A change to some fields of a block definition, for a family's members. Tags are added
/// to the member's own. (`tint: Some(None)` removes a tint: plain `None` means "no
/// change".)
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BlockPatch {
    pub render: Option<RenderKind>,
    pub solid: Option<bool>,
    pub textures: Option<Textures>,
    pub tint: Option<BlockTint>,
    pub tint_top_only: Option<bool>,
    pub light: Option<u8>,
    pub light_filter: Option<u8>,
    pub hardness: Option<f32>,
    pub sound: Option<Sound>,
    pub material: Option<String>,
    pub drops: Option<String>,
    pub drop_count: Option<u8>,
    pub color: Option<[u8; 3]>,
    pub tags: Vec<String>,
    pub icon: Option<String>,
    pub rarity: Option<Rarity>,
    pub variants: Option<Vec<Variant>>,
    pub support: Option<Support>,
    pub item: Option<bool>,
    pub action: Option<String>,
    pub model: Option<PlantModel>,
    pub plant_height: Option<f32>,
}

impl BlockPatch {
    fn apply(&self, def: &mut BlockDef) {
        let BlockPatch {
            render,
            solid,
            textures,
            tint,
            tint_top_only,
            light,
            light_filter,
            hardness,
            sound,
            material,
            drops,
            drop_count,
            color,
            tags,
            icon,
            rarity,
            variants,
            support,
            item,
            action,
            model,
            plant_height,
        } =
            self.clone();
        render.into_iter().for_each(|v| def.render = v);
        solid.into_iter().for_each(|v| def.solid = v);
        textures.into_iter().for_each(|v| def.textures = v);
        tint.into_iter().for_each(|v| def.tint = v);
        tint_top_only.into_iter().for_each(|v| def.tint_top_only = v);
        light.into_iter().for_each(|v| def.light = v);
        light_filter.into_iter().for_each(|v| def.light_filter = v);
        hardness.into_iter().for_each(|v| def.hardness = v);
        sound.into_iter().for_each(|v| def.sound = v);
        material.into_iter().for_each(|v| def.material = Some(v));
        drops.into_iter().for_each(|v| def.drops = Some(v));
        drop_count.into_iter().for_each(|v| def.drop_count = v);
        color.into_iter().for_each(|v| def.color = v);
        def.tags.extend(tags);
        icon.into_iter().for_each(|v| def.icon = Some(v));
        rarity.into_iter().for_each(|v| def.rarity = v);
        variants.into_iter().for_each(|v| def.variants = v);
        support.into_iter().for_each(|v| def.support = Some(v));
        item.into_iter().for_each(|v| def.item = v);
        action.into_iter().for_each(|v| def.action = Some(v));
        model.into_iter().for_each(|v| def.model = v);
        plant_height.into_iter().for_each(|v| def.plant_height = v);
    }
}

/// One use of a template: the whole set of blocks for one name.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FamilyDef {
    /// `wood`, or `base:wood` for another package's template.
    pub template: String,
    /// Replaces `{name}` in the template.
    pub name: String,
    /// Added to every member's tags.
    pub tags: Vec<String>,
    /// Applied to every member.
    pub all: BlockPatch,
    /// Applied to single members, by their name (`cherry_leaves`).
    pub members: BTreeMap<String, BlockPatch>,
    /// Members to leave out, by name.
    pub without: Vec<String>,
    /// Makes the family once for every dye (see [`DyeDef`]) instead of once for `name`:
    /// each takes the dye's name for `{name}`, and its colour as every member's
    /// `tint: Fixed(..)` and `color` (a member may still say otherwise). `members` and
    /// `without` are not allowed, since every dye's members have other names.
    pub dyes: bool,
}

/// One dye of the palette dyed families are made over: a name (its language names are
/// `family.<package>.<name>`) and the colour it multiplies greyscale textures by.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DyeDef {
    pub name: String,
    pub rgb: [u8; 3],
}

/// A package's `blocks.ron`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BlockFile {
    /// Families' templates by name: definitions with `{name}` in them.
    pub templates: BTreeMap<String, Vec<BlockDef>>,
    pub blocks: Vec<BlockDef>,
    /// Made after `blocks`, in order.
    pub families: Vec<FamilyDef>,
    /// The dyes families with `dyes: true` are made over. Any package may add some; every
    /// package's dyes are one palette.
    pub dyes: Vec<DyeDef>,
    /// Blocks and items that were merged into another or renamed, old name to new: an older
    /// save's `raw_rabbit` loads as `raw_small_game` (see [`BlockRegistry::item_named`]).
    pub renamed: BTreeMap<String, String>,
}

/// One look of a block, for generators choosing what to place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub block: BlockId,
    /// How many of the block breaking this look gives.
    pub size: u8,
    pub rarity: Rarity,
}

/// Runtime properties of a block, precomputed for hot loops.
#[derive(Clone, Copy, Debug)]
pub struct BlockInfo {
    pub render: RenderKind,
    pub solid: bool,
    /// Hides adjacent faces and fully blocks light.
    pub occludes: bool,
    pub emission: u8,
    /// Light lost when light enters this block, 1..=15 (15 = opaque).
    pub attenuation: u8,
    pub layers: [u16; 6],
    pub tints: [Tint; 6],
    pub hardness: f32,
    pub sound: Sound,
    /// What breaking the block gives; air for nothing.
    pub drops: BlockId,
    /// A block placed into this cell replaces it (air, fluids, plants).
    pub replaceable: bool,
    /// The neighbour it rests on and goes with (see [`Support`]).
    pub support: Support,
    /// How many of `drops` breaking it gives.
    pub drop_count: u8,
    /// The fluid state this block is, for water, lava and their flowing states.
    pub fluid: Option<FluidState>,
    /// Whether holding it and using it places it: false for items (see [`BlockDef::item`]).
    pub placeable: bool,
    /// Hunger points eating it fills, from its tags (see [`crate::vitals::food_value`]);
    /// 0 for what is not food.
    pub food: u8,
    /// A plant's shape and height (see [`BlockDef::model`]).
    pub model: PlantModel,
    pub plant_height: f32,
    /// Which way it faces, for a block made in four facings (see [`BlockDef::facing`]).
    pub facing: Option<Facing>,
    /// The block placed instead against a block's side, facing north (see
    /// [`BlockDef::wall`] and [`BlockRegistry::facing`]).
    pub wall: Option<BlockId>,
    /// What it turns into when used, facing north (see [`BlockDef::swaps`] and
    /// [`BlockRegistry::swapped`]).
    pub swaps: Option<BlockId>,
    /// The block placed above with it, facing north (see [`BlockDef::upper`] and
    /// [`BlockRegistry::upper`]).
    pub upper: Option<BlockId>,
    /// Its shape, made ready for its facing (see [`BlockRegistry::shape`]), for a block
    /// drawn as one.
    pub shape: Option<u16>,
    /// A ladder, climbed by a player whose feet are in its cell (`CLIMBABLE_TAG`).
    pub climbable: bool,
}

/// A texture referenced by block definitions; its index is the texture array layer.
#[derive(Clone, Debug)]
pub struct TextureRef {
    pub name: String,
    /// A texture it is drawn over (its block's host's, see [`BlockDef::host`]).
    pub under: Option<String>,
    pub color: [u8; 3],
    /// How the first block using the texture is drawn, so placeholders can match its shape.
    pub render: RenderKind,
    /// A colour its texels are multiplied by (the block's [`BlockTint::Fixed`]); the same texture
    /// in two colours is two layers.
    pub tint: Option<[u8; 3]>,
}

#[derive(Debug)]
pub struct BlockRegistry {
    defs: Vec<BlockDef>,
    info: Vec<BlockInfo>,
    by_name: FxHashMap<String, BlockId>,
    textures: Vec<TextureRef>,
    /// The shapes blocks drawn as boxes take, each in one facing, once however many blocks
    /// take it (by [`BlockInfo::shape`]).
    shapes: Vec<crate::shape::Shape>,
    /// Where each shape's quads start in all shapes' quads, in order.
    shape_starts: Vec<u32>,
    /// The texture layer of each quad of a block's shape, by block.
    shape_layers: Vec<Vec<u16>>,
    /// What each shape stops movement with: its boxes' unturned bounds, by shape.
    shape_collision: Vec<Vec<crate::physics::Aabb>>,
    shape_hull: Vec<Vec<crate::physics::Aabb>>,
    /// By block, whether it is only a form another takes when placed (`placement_form`).
    placement_forms: Vec<bool>,
    /// Every block's looks: itself and its variants (shared by the variants).
    looks: Vec<Vec<Look>>,
    fluids: Vec<Fluid>,
    /// Old names older saves may hold, each the block it is now (`BlockFile::renamed`).
    renamed: FxHashMap<String, BlockId>,
}

#[derive(Debug)]
pub struct RegistryError(pub String);

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RegistryError {}

impl BlockRegistry {
    pub const AIR: &'static str = "core:air";

    /// Parses a package's `blocks.ron`. Unqualified names (`stone`) are placed in the
    /// package's `namespace` (`base:stone`).
    pub fn from_ron(namespace: &str, source: &str) -> Result<Self, RegistryError> {
        Self::new(Self::parse(namespace, source)?)
    }

    /// Parses one package's block definitions and makes its families, qualifying names
    /// with its namespace.
    pub fn parse(namespace: &str, source: &str) -> Result<Vec<BlockDef>, RegistryError> {
        Self::expand(vec![(namespace.to_owned(), Self::read(namespace, source)?)])
    }

    /// Reads a package's `blocks.ron` as written.
    pub fn read(namespace: &str, source: &str) -> Result<BlockFile, RegistryError> {
        // `drops: "dirt"` as well as `drops: Some("dirt")`.
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/blocks.ron: {e}")))
    }

    /// Every package's blocks, then its families, in package order, with names qualified
    /// by namespace. Templates are found in any package, so the order packages load in
    /// never matters to which templates exist.
    pub fn expand(packages: Vec<(String, BlockFile)>) -> Result<Vec<BlockDef>, RegistryError> {
        let templates: FxHashMap<String, Vec<BlockDef>> = packages
            .iter()
            .flat_map(|(namespace, file)| file.templates.iter().map(move |(name, members)| (format!("{namespace}:{name}"), members.clone())))
            .collect();
        let mut dyes: Vec<DyeDef> = Vec::new();
        for dye in packages.iter().flat_map(|(_, file)| &file.dyes) {
            if dyes.iter().any(|d| d.name == dye.name) {
                return Err(RegistryError(format!("dye `{}` is made twice", dye.name)));
            }
            dyes.push(dye.clone());
        }
        let mut defs = Vec::new();
        for (namespace, file) in packages {
            for mut def in file.blocks {
                def.qualify(&namespace);
                defs.push(def);
            }
            for family in file.families {
                let error = |message: String| RegistryError(format!("{namespace}/blocks.ron: family `{}`: {message}", family.name));
                let key = if family.template.contains(':') { family.template.clone() } else { format!("{namespace}:{}", family.template) };
                let template = templates.get(&key).ok_or_else(|| error(format!("no template `{key}`")))?;
                if family.dyes && (!family.members.is_empty() || !family.without.is_empty()) {
                    return Err(error("a family over the dyes has no `members` or `without`: its members have a name for every dye".into()));
                }
                // One family, or one for every dye.
                let fills: Vec<(&str, Option<[u8; 3]>)> =
                    if family.dyes { dyes.iter().map(|dye| (dye.name.as_str(), Some(dye.rgb))).collect() } else { vec![(family.name.as_str(), None)] };
                if family.dyes && fills.is_empty() {
                    return Err(error("it is made over the dyes, and no package has any".into()));
                }
                for (fill, rgb) in fills {
                    let mut names = Vec::new();
                    for member in template {
                        let mut def = member.clone();
                        def.substitute(fill);
                        names.push(def.name.clone());
                        if family.without.contains(&def.name) {
                            continue;
                        }
                        if let Some([r, g, b]) = rgb {
                            def.tint = BlockTint::Fixed(r, g, b);
                            def.color = [r, g, b];
                        }
                        def.tags.extend(family.tags.iter().cloned());
                        family.all.apply(&mut def);
                        if let Some(patch) = family.members.get(&def.name) {
                            patch.apply(&mut def);
                        }
                        def.family = Some(fill.to_owned());
                        def.qualify(&namespace);
                        defs.push(def);
                    }
                    if let Some(unknown) = family.members.keys().chain(&family.without).find(|n| !names.contains(n)) {
                        return Err(error(format!("`{unknown}` is not one of its members ({})", names.join(", "))));
                    }
                }
            }
        }
        Ok(defs)
    }

    /// Builds the registry from fully qualified definitions, making each variant its own
    /// block right after the block it varies. `core:air` is always id 0. Read so, without
    /// the packages' shapes (blocks alone, for a test or a tool), a block drawn as a shape
    /// has none and draws nothing; the game loads them with [`Self::with_shapes`].
    pub fn new(content: Vec<BlockDef>) -> Result<Self, RegistryError> {
        Self::build(content, None)
    }

    /// [`Self::new`], with the shapes blocks drawn as boxes take (`render: Model`), each
    /// checked as it is made ready (see [`crate::shape::compile`]).
    pub fn with_shapes(content: Vec<BlockDef>, shape_defs: &crate::shape::Shapes) -> Result<Self, RegistryError> {
        Self::build(content, Some(shape_defs))
    }

    fn build(content: Vec<BlockDef>, shape_defs: Option<&crate::shape::Shapes>) -> Result<Self, RegistryError> {
        let air = BlockDef {
            name: Self::AIR.into(),
            render: RenderKind::Invisible,
            solid: false,
            hardness: -1.0,
            ..Default::default()
        };
        let mut defs = vec![air];
        // The fluids, each with the index of its source block.
        let mut fluid_sources = Vec::new();
        // A block that comes upside down is made both ways up, each then in its facings.
        let content: Vec<BlockDef> = content
            .into_iter()
            .flat_map(|def| {
                let flipped = def.flips.then(|| BlockDef {
                    name: format!("{}{FLIPPED}", def.name),
                    drops: Some(def.drops.clone().unwrap_or_else(|| def.name.clone())),
                    flips: false,
                    flipped: true,
                    ..def.clone()
                });
                std::iter::once(def).chain(flipped)
            })
            .collect();
        for def in content {
            let base = def.clone();
            if def.fluid.is_some() {
                if !def.variants.is_empty() {
                    return Err(RegistryError(format!("fluid `{}` cannot have variants", def.name)));
                }
                fluid_sources.push(defs.len());
                defs.extend(fluid::state_defs(&def));
                continue;
            }
            if def.facing {
                defs.push(BlockDef { faces: Some(Facing::North), ..def });
                for facing in [Facing::East, Facing::South, Facing::West] {
                    let mut turned = BlockDef {
                        name: format!("{}{}", base.name, facing.suffix()),
                        drops: Some(base.drops.clone().unwrap_or_else(|| base.name.clone())),
                        faces: Some(facing),
                        ..base.clone()
                    };
                    turned.particles.iter_mut().for_each(|emission| emission.at = facing.turn(emission.at));
                    defs.push(turned);
                }
                if !base.variants.is_empty() {
                    return Err(RegistryError(format!("`{}` comes in facings, so it cannot have variants too", base.name)));
                }
                continue;
            }
            defs.push(def);
            for variant in &base.variants {
                defs.push(BlockDef {
                    name: format!("{}_{}", base.name, variant.name),
                    textures: variant.textures.clone(),
                    drops: Some(base.drops.clone().unwrap_or_else(|| base.name.clone())),
                    drop_count: variant.size,
                    variants: Vec::new(),
                    variant_of: Some(base.name.clone()),
                    model: variant.model.unwrap_or(base.model),
                    ..base.clone()
                });
            }
        }
        if defs.len() > u16::MAX as usize {
            return Err(RegistryError("too many blocks".into()));
        }
        if fluid_sources.len() > u8::MAX as usize {
            return Err(RegistryError("too many fluids".into()));
        }

        let mut by_name = FxHashMap::default();
        for (i, def) in defs.iter().enumerate() {
            if by_name.insert(def.name.clone(), BlockId(i as u16)).is_some() {
                return Err(RegistryError(format!("duplicate block `{}`", def.name)));
            }
            for emission in &def.particles {
                if emission.range.is_some_and(|range| !(1.0..=MAX_EMISSION_RANGE).contains(&range)) {
                    return Err(RegistryError(format!("block `{}` gives off particles seen from {:?} blocks; from 1 to {MAX_EMISSION_RANGE}", def.name, emission.range)));
                }
                if let Some((tag, reach)) = &emission.over
                    && (tag.is_empty() || !(1..=MAX_EMISSION_OVER).contains(reach))
                {
                    return Err(RegistryError(format!("block `{}` gives off particles over `{tag}` {reach} blocks down; a tag, from 1 to {MAX_EMISSION_OVER} down", def.name)));
                }
            }
        }

        let sources: Vec<BlockId> = fluid_sources.iter().map(|&i| BlockId(i as u16)).collect();
        let fluids = sources
            .iter()
            .enumerate()
            .map(|(i, &source)| Fluid::compile(FluidId(i as u8), source, &defs[source.0 as usize], &by_name, &sources))
            .collect::<Result<Vec<_>, _>>()?;
        let mut states = vec![None; defs.len()];
        for fluid in &fluids {
            for (block, state) in fluid.states() {
                states[block.0 as usize] = Some(state);
            }
        }

        let mut textures: Vec<TextureRef> = Vec::new();
        let mut texture_index = FxHashMap::<(String, Option<String>, Option<[u8; 3]>), u16>::default();
        let mut info = Vec::with_capacity(defs.len());
        let mut shapes: Vec<crate::shape::Shape> = Vec::new();
        let mut shape_starts = Vec::new();
        let mut shape_layers = vec![Vec::new(); defs.len()];
        let mut shape_quads = 0;
        for (index, (def, &fluid)) in defs.iter().zip(&states).enumerate() {
            let shape = match (def.render, &def.shape, shape_defs) {
                // Read without the shapes: it has none.
                (RenderKind::Model, Some(_), None) => None,
                (RenderKind::Model, Some(name), Some(shape_defs)) => {
                    let shape_def = shape_defs.defs.get(name).ok_or_else(|| RegistryError(format!("block `{}` takes unknown shape `{name}`", def.name)))?;
                    let limits = shape_defs.limits;
                    if limits.boxes == 0 || limits.quads == 0 {
                        return Err(RegistryError(format!("block `{}` takes shape `{name}`, but no package sets the shape limits (`limits` in shapes.ron)", def.name)));
                    }
                    let turned_over;
                    let shape_def = if def.flipped {
                        turned_over = shape_def.upside_down();
                        &turned_over
                    } else {
                        shape_def
                    };
                    let (shape, names) = crate::shape::compile(name, &def.name, shape_def, &def.textures, def.faces.unwrap_or_default(), limits)?;
                    shape_layers[index] = names
                        .into_iter()
                        .map(|name| {
                            *texture_index.entry((name.clone(), None, def.tint.baked())).or_insert_with(|| {
                                textures.push(TextureRef { name, under: None, color: def.color, render: def.render, tint: def.tint.baked() });
                                (textures.len() - 1) as u16
                            })
                        })
                        .collect();
                    // Blocks of one shape (the stairs of every wood) share its quads.
                    let found = match shapes.iter().position(|s| *s == shape) {
                        Some(found) => found,
                        None => {
                            shape_starts.push(shape_quads as u32);
                            shape_quads += shape.quads.len();
                            if shape_quads > limits.quads {
                                return Err(RegistryError(format!(
                                    "block `{}`, shape `{name}`: shapes have more than the {} quads they may have in all",
                                    def.name, limits.quads
                                )));
                            }
                            shapes.push(shape);
                            shapes.len() - 1
                        }
                    };
                    Some(found as u16)
                }
                (RenderKind::Model, None, _) => return Err(RegistryError(format!("block `{}` is drawn as a shape (`render: Model`) but names none", def.name))),
                (_, None, _) if def.flips => return Err(RegistryError(format!("block `{}` comes upside down (`flips`) but is not drawn as a shape", def.name))),
                (_, Some(_), _) => return Err(RegistryError(format!("block `{}` names a shape but is not drawn as one (`render: Model`)", def.name))),
                _ => None,
            };
            let host = match &def.host {
                Some(name) => match by_name.get(name) {
                    Some(&id) => Some(&defs[id.0 as usize]),
                    None => return Err(RegistryError(format!("block `{}` lies in unknown block `{name}`", def.name))),
                },
                None => None,
            };
            let mut layers = [0u16; 6];
            let mut tints = [Tint::None; 6];
            for face in Face::ALL {
                if let Some(name) = def.textures.face(face) {
                    let under = host.and_then(|host| host.textures.face(face)).map(str::to_owned);
                    layers[face as usize] = *texture_index.entry((name.to_owned(), under.clone(), def.tint.baked())).or_insert_with(|| {
                        textures.push(TextureRef { name: name.to_owned(), under, color: def.color, render: def.render, tint: def.tint.baked() });
                        (textures.len() - 1) as u16
                    });
                }
                if !def.tint_top_only || face == Face::PosY {
                    tints[face as usize] = def.tint.drawn();
                }
            }
            let drops = match &def.drops {
                Some(name) => *by_name
                    .get(name)
                    .ok_or_else(|| RegistryError(format!("`{}` drops unknown block `{name}`", def.name)))?,
                None => by_name[&def.name],
            };
            if def.ward != 0.0 && !(WARD_LEAST..=WARD_MOST).contains(&def.ward) {
                return Err(RegistryError(format!("`{}` wards {} blocks round it: 0 for none, else {WARD_LEAST} to {WARD_MOST}", def.name, def.ward)));
            }
            let occludes = def.render == RenderKind::Opaque || shape.is_some_and(|s| shapes[s as usize].is_full_cube());
            info.push(BlockInfo {
                render: def.render,
                solid: def.solid,
                occludes,
                emission: def.light.min(15),
                attenuation: if occludes { 15 } else { (1 + def.light_filter).min(15) },
                layers,
                tints,
                hardness: def.hardness,
                sound: def.sound,
                drops,
                replaceable: !def.solid
                    && (fluid.is_some() || matches!(def.render, RenderKind::Invisible | RenderKind::Translucent | RenderKind::Plant)),
                support: def.support.unwrap_or(if def.render == RenderKind::Plant { Support::Below } else { Support::None }),
                drop_count: def.drop_count,
                fluid,
                placeable: !def.item && def.action.is_none(),
                food: crate::vitals::food_value(&def.tags),
                model: def.model,
                plant_height: def.plant_height,
                facing: def.faces,
                wall: match &def.wall {
                    Some(name) => {
                        let wall = *by_name.get(name).ok_or_else(|| RegistryError(format!("`{}` goes on walls as unknown block `{name}`", def.name)))?;
                        if defs[wall.0 as usize].faces.is_none() {
                            return Err(RegistryError(format!("`{}` goes on walls as `{name}`, which has no facings", def.name)));
                        }
                        // Placed against a side, it must rest on that side, and go with it.
                        if defs[wall.0 as usize].support != Some(Support::Wall) {
                            return Err(RegistryError(format!("`{}` goes on walls as `{name}`, which does not rest on a wall (`support: Wall`)", def.name)));
                        }
                        Some(wall)
                    }
                    None => None,
                },
                swaps: match &def.swaps {
                    Some(name) => {
                        let to = *by_name.get(name).ok_or_else(|| RegistryError(format!("`{}` swaps for unknown block `{name}`", def.name)))?;
                        if defs[to.0 as usize].facing != def.facing {
                            return Err(RegistryError(format!("`{}` swaps for `{name}`, which {} facings", def.name, if def.facing { "has no" } else { "comes in" })));
                        }
                        Some(to)
                    }
                    None => None,
                },
                upper: match &def.upper {
                    Some(name) => {
                        let up = *by_name.get(name).ok_or_else(|| RegistryError(format!("`{}` has unknown upper block `{name}`", def.name)))?;
                        // It stands on its lower half and goes with it.
                        if defs[up.0 as usize].support != Some(Support::Below) {
                            return Err(RegistryError(format!("`{}` has upper block `{name}`, which does not rest on it (`support: Below`)", def.name)));
                        }
                        if defs[up.0 as usize].facing != def.facing {
                            return Err(RegistryError(format!("`{}` has upper block `{name}`, which {} facings", def.name, if def.facing { "has no" } else { "comes in" })));
                        }
                        Some(up)
                    }
                    None => None,
                },
                shape,
                climbable: def.has_tag(CLIMBABLE_TAG),
            });
        }
        if textures.len() > 4096 {
            return Err(RegistryError("too many textures (max 4096)".into()));
        }

        let mut looks: Vec<Vec<Look>> = (0..defs.len())
            .map(|i| vec![Look { block: BlockId(i as u16), size: defs[i].drop_count, rarity: Rarity::Common }])
            .collect();
        for (i, def) in defs.iter().enumerate() {
            for variant in &def.variants {
                let id = by_name[&format!("{}_{}", def.name, variant.name)];
                looks[i].push(Look { block: id, size: variant.size, rarity: variant.rarity });
            }
        }
        for i in 0..defs.len() {
            if let Some(base) = &defs[i].variant_of {
                let base = *by_name.get(base).ok_or_else(|| RegistryError(format!("`{}` is a variant of unknown block `{base}`", defs[i].name)))?;
                looks[i] = looks[base.0 as usize].clone();
            }
        }

        let mut placement_forms: Vec<bool> = defs.iter().map(|def| def.variant_of.is_some() || def.form_of().is_some()).collect();
        for wall in info.iter().filter_map(|info| info.wall) {
            placement_forms[wall.0 as usize] = true;
        }
        // An upper half is only ever placed with its lower; a form swapped to that drops
        // another block (an open door, dropping the door) is only ever swapped to.
        let forms: Vec<BlockId> = info
            .iter()
            .flat_map(|block| block.upper.into_iter().chain(block.swaps.filter(|&to| info[to.0 as usize].drops != to)))
            .collect();
        for form in forms {
            placement_forms[form.0 as usize] = true;
            for facing in Facing::ALL {
                if let Some(&id) = by_name.get(&format!("{}{}", defs[form.0 as usize].name, facing.suffix())) {
                    placement_forms[id.0 as usize] = true;
                }
            }
        }
        // Nothing drops a plant whose item merged into another: that is never held, never
        // stacks with its kind and shows in no catalogue. It drops what it merged into.
        for (i, block) in info.iter().enumerate() {
            let dropped = block.drops;
            if defs[dropped.0 as usize].tags.iter().any(|t| t == "merged") {
                let into = &defs[info[dropped.0 as usize].drops.0 as usize].name;
                return Err(RegistryError(format!("`{}` drops merged-away `{}`; drop `{into}` (item_of) instead", defs[i].name, defs[dropped.0 as usize].name)));
            }
        }
        // A window lets light through: a pane, a lattice, a frame or a slit, anything a
        // draught comes through, and anything lit by the light through it. None of them is
        // a whole cube or dims light as a shut shutter does.
        for (def, block) in defs.iter().zip(&info) {
            let tag = def.tags.iter().find(|t| LIGHT_THROUGH_TAGS.contains(&t.as_str()));
            if (def.lit_through || tag.is_some()) && (block.occludes || def.light_filter > MAX_WINDOW_FILTER) {
                let why = tag.map_or("`lit_through`".to_owned(), |t| format!("tagged `{t}`"));
                return Err(RegistryError(format!(
                    "`{}` is a window ({why}) but keeps the light out: a window is no whole cube, and its `light_filter` is {MAX_WINDOW_FILTER} at most, not {}",
                    def.name, def.light_filter
                )));
            }
        }
        // A window glows by the light through it, and glowing is its drawing's (the client
        // marks the texture layers of a `lit_through` block's faces): a drawing a block that
        // does not glow shares, on a face or a box, would light it too.
        fn textured<'a>(def: &'a BlockDef, block: &'a BlockInfo) -> impl Iterator<Item = u16> + 'a {
            Face::ALL.into_iter().filter(move |&f| def.textures.face(f).is_some()).map(move |f| block.layers[f as usize])
        }
        let glowing: FxHashMap<u16, &str> = defs.iter().zip(&info).filter(|(def, _)| def.lit_through).flat_map(|(def, block)| textured(def, block).map(move |layer| (layer, def.name.as_str()))).collect();
        for (i, (def, block)) in defs.iter().zip(&info).enumerate().filter(|(_, (def, _))| !def.lit_through) {
            if let Some(window) = textured(def, block).chain(shape_layers[i].iter().copied()).find_map(|layer| glowing.get(&layer)) {
                return Err(RegistryError(format!(
                    "`{}` is drawn as window `{window}` is, which glows by the light through it (`lit_through`), so it would glow too: give the window a drawing of its own",
                    def.name
                )));
            }
        }
        // A railing keeps anyone from walking off what it edges: solid, and a block high so
        // nobody steps onto it. A landing is a floor level with a stair's top: a box across
        // the whole top of its cell.
        for (def, block) in defs.iter().zip(&info) {
            let bounds: &[(glam::Vec3, glam::Vec3)] = match (block.solid, block.shape) {
                (false, _) => &[],
                (true, Some(s)) => &shapes[s as usize].bounds,
                (true, None) => &[(glam::Vec3::ZERO, glam::Vec3::ONE)],
            };
            let full_top = bounds.iter().any(|&(min, max)| max.y >= 1.0 && min.x <= 0.0 && min.z <= 0.0 && max.x >= 1.0 && max.z >= 1.0);
            if def.has_tag(RAILING_TAG) && !bounds.iter().any(|&(_, max)| max.y >= 1.0) {
                return Err(RegistryError(format!("`{}` is a railing (`{RAILING_TAG}`) but nobody would be kept back by it: a railing is solid and a block high", def.name)));
            }
            if def.has_tag(CHIMNEY_PIECE_TAG) && !block.solid {
                return Err(RegistryError(format!("`{}` is a chimney piece (`{CHIMNEY_PIECE_TAG}`) but not solid: a flue keeps a hearth's fire in", def.name)));
            }
            if def.has_tag(CLIMBABLE_TAG) && !block.solid {
                return Err(RegistryError(format!("`{}` is climbable (`{CLIMBABLE_TAG}`) but not solid: a ladder is climbed by pressing against it", def.name)));
            }
            if def.has_tag(LANDING_TAG) && !full_top {
                return Err(RegistryError(format!("`{}` is a landing (`{LANDING_TAG}`) but no floor level with a stair's top: a landing is solid across the whole top of its cell", def.name)));
            }
        }
        let shape_collision = shapes.iter().map(|s| s.bounds.iter().map(|&(min, max)| crate::physics::Aabb { min, max }).collect()).collect();
        let shape_hull = shapes.iter().map(|s| s.hull.iter().map(|&(min, max)| crate::physics::Aabb { min, max }).collect()).collect();
        // A coating names blocks that are, held as an item, and turns it into another.
        for def in &defs {
            let Some(coat) = &def.coat else { continue };
            for (what, name) in [("coated with", &coat.with), ("coats into", &coat.into)] {
                if !by_name.contains_key(name) {
                    return Err(RegistryError(format!("`{}` is {what} unknown block `{name}`", def.name)));
                }
            }
            if coat.into == def.name || defs[by_name[&coat.into].0 as usize].coat.is_some() {
                return Err(RegistryError(format!("`{}` coats into `{}`, which is itself or coats again", def.name, coat.into)));
            }
        }
        Ok(Self { defs, info, by_name, textures, shapes, shape_starts, shape_layers, shape_collision, shape_hull, placement_forms, looks, fluids, renamed: FxHashMap::default() })
    }

    /// A block's looks for a generator to choose from: the block itself first, then its
    /// variants, each with its size and rarity. A variant gives the same list as the block
    /// it varies; a block without variants has one look.
    pub fn looks(&self, id: BlockId) -> &[Look] {
        &self.looks[id.0 as usize]
    }

    /// Blocks carrying `tag`, in id order, leaving out variants (reach those through
    /// [`Self::looks`]) and fluids' other states.
    pub fn tagged<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = BlockId> + 'a {
        (0..self.defs.len())
            .map(|i| BlockId(i as u16))
            .filter(move |&id| {
                let def = &self.defs[id.0 as usize];
                def.variant_of.is_none() && self.info(id).fluid.is_none_or(|f| self.fluids[f.fluid.0 as usize].source == id) && def.has_tag(tag)
            })
    }

    /// Whether a block is less than a whole block: drawn as a shape (a slab, stairs, a post,
    /// a column, a headstone), whatever its tags. See [`Self::answers_whole`].
    pub fn is_piece(&self, id: BlockId) -> bool {
        self.info(id).shape.is_some()
    }

    /// Blocks that answer `tag:TAG` where whole blocks are counted (a multiblock's cells):
    /// every block with the tag, but a piece (see [`Self::is_piece`]) only when no whole
    /// block has it too, so cobblestone slabs never stand in for cobblestone (six slabs come
    /// from three blocks) while a tag only pieces carry (`beam`) still takes them.
    pub fn answers_whole(&self, tag: &str) -> Vec<BlockId> {
        let all: Vec<BlockId> = (0..self.defs.len() as u16).map(BlockId).filter(|&id| self.def(id).has_tag(tag)).collect();
        let whole = all.iter().any(|&id| !self.is_piece(id));
        all.into_iter().filter(|&id| !whole || !self.is_piece(id)).collect()
    }

    /// The fluid state a block is, if it is one.
    #[inline]
    pub fn fluid(&self, id: BlockId) -> Option<FluidState> {
        self.info[id.0 as usize].fluid
    }

    /// Every fluid, by id.
    pub fn fluids(&self) -> &[Fluid] {
        &self.fluids
    }

    #[inline]
    pub fn fluid_def(&self, id: FluidId) -> &Fluid {
        &self.fluids[id.0 as usize]
    }

    #[inline]
    pub fn info(&self, id: BlockId) -> &BlockInfo {
        &self.info[id.0 as usize]
    }

    pub fn def(&self, id: BlockId) -> &BlockDef {
        &self.defs[id.0 as usize]
    }

    pub fn id(&self, name: &str) -> Option<BlockId> {
        self.by_name.get(name).copied()
    }

    /// The block an older save's `name` is today: itself, or what it was renamed or merged
    /// into (`BlockFile::renamed`).
    pub fn saved_id(&self, name: &str) -> Option<BlockId> {
        self.id(name).or_else(|| self.renamed.get(name).copied())
    }

    /// The item an older save's `name` is today: what a block tagged `merged` drops (a
    /// wild oregano picked in an old world is kitchen herbs now), what a renamed item is
    /// called now (raw rabbit is raw small game), else the block itself.
    pub fn item_named(&self, name: &str) -> Option<BlockId> {
        Some(self.item_of(self.saved_id(name)?))
    }

    /// Every package's renamed blocks (`BlockFile::renamed`), both names qualified by the
    /// package's namespace, ready for [`Self::rename`].
    pub fn renames(packages: &[(String, BlockFile)]) -> Vec<(String, String)> {
        let qualify = |namespace: &str, name: &str| if name.contains(':') { name.to_owned() } else { format!("{namespace}:{name}") };
        packages.iter().flat_map(|(namespace, file)| file.renamed.iter().map(move |(old, new)| (qualify(namespace, old), qualify(namespace, new)))).collect()
    }

    /// Takes old names for blocks, each to the block it is now. An old name must not be a
    /// block's name still, and the new one must be: a rename to nothing would lose what
    /// older saves hold.
    pub fn rename(&mut self, renames: Vec<(String, String)>) -> Result<(), RegistryError> {
        for (old, new) in renames {
            if self.by_name.contains_key(&old) {
                return Err(RegistryError(format!("`{old}` is renamed to `{new}`, but a block is still called `{old}`")));
            }
            let Some(id) = self.id(&new) else {
                return Err(RegistryError(format!("`{old}` is renamed to unknown block `{new}`")));
            };
            if self.renamed.insert(old.clone(), id).is_some() {
                return Err(RegistryError(format!("`{old}` is renamed twice")));
            }
        }
        Ok(())
    }

    /// The item `id` is held as: what a block tagged `merged` drops, else itself.
    pub fn item_of(&self, id: BlockId) -> BlockId {
        if self.merged_away(id) { self.info[id.0 as usize].drops } else { id }
    }

    /// Whether `id` is a wild plant whose item merged into another (see `item_of`): it
    /// stays in the world, but is never held, so catalogues, the creative inventory, the
    /// journal and the recipe book leave it out.
    pub fn merged_away(&self, id: BlockId) -> bool {
        self.def(id).has_tag("merged")
    }

    /// Whether a catalogue lists `id`: what a player could hold, not air, nor another look
    /// or facing of a block (`placement_form`), nor a plant whose item merged.
    pub fn listed(&self, id: BlockId) -> bool {
        !id.is_air() && !self.placement_form(id) && !self.merged_away(id)
    }

    /// Whether a player could hold `id` as an item, so the creative catalogue and the
    /// inventory search list it: `listed`, and not a fluid's other state (its source stays),
    /// nor invisible, nor a stage of a growing crop (its seeds stay).
    pub fn holdable(&self, id: BlockId) -> bool {
        self.listed(id)
            && self.def(id).render != RenderKind::Invisible
            && self.fluid(id).is_none_or(|state| self.fluids[state.fluid.0 as usize].source == id)
            && !self.def(id).has_tag("crop")
    }

    /// Whether the block is only ever a form another takes when placed, never held: another
    /// look of a block (`variant_of`), a block turned to face east, south or west or upside
    /// down, or what a block becomes against a wall (a wall torch). Catalogues list the
    /// block itself.
    pub fn placement_form(&self, id: BlockId) -> bool {
        self.placement_forms[id.0 as usize]
    }

    /// `block` upside down (see [`BlockDef::flips`]), in the same facing; none if it does
    /// not come that way or already is.
    pub fn flipped(&self, block: BlockId) -> Option<BlockId> {
        let def = self.def(block);
        if def.flipped || !def.flips {
            return None;
        }
        let base = def.form_of().unwrap_or(&def.name);
        let up = self.id(&format!("{base}{FLIPPED}"))?;
        Some(self.info(block).facing.map_or(up, |facing| self.facing(up, facing)))
    }

    /// `block`, made in four facings (see [`BlockDef::facing`]), turned to face `facing`;
    /// `block` itself if it has no facings.
    pub fn facing(&self, block: BlockId, facing: Facing) -> BlockId {
        let Some(now) = self.info(block).facing else { return block };
        let name = &self.def(block).name;
        let base = name.strip_suffix(now.suffix()).unwrap_or(name);
        self.id(&format!("{base}{}", facing.suffix())).unwrap_or(block)
    }

    /// What coats `block` in place and what it becomes (see [`BlockDef::coat`]).
    pub fn coating(&self, block: BlockId) -> Option<(BlockId, BlockId)> {
        let coat = self.def(block).coat.as_ref()?;
        Some((*self.by_name.get(&coat.with)?, *self.by_name.get(&coat.into)?))
    }

    /// What `block` turns into when used (see [`BlockDef::swaps`]), in its facing.
    pub fn swapped(&self, block: BlockId) -> Option<BlockId> {
        let to = self.info(block).swaps?;
        Some(self.info(block).facing.map_or(to, |facing| self.facing(to, facing)))
    }

    /// The block placed above `block` with it (see [`BlockDef::upper`]), in its facing.
    pub fn upper(&self, block: BlockId) -> Option<BlockId> {
        let up = self.info(block).upper?;
        Some(self.info(block).facing.map_or(up, |facing| self.facing(up, facing)))
    }

    /// Like [`Self::id`] but for names the engine or generator cannot work without.
    pub fn expect(&self, name: &str) -> BlockId {
        self.id(name).unwrap_or_else(|| panic!("content is missing required block `{name}`"))
    }

    /// Every block's id, in registration order.
    pub fn ids(&self) -> impl Iterator<Item = BlockId> + '_ {
        (0..self.defs.len() as u16).map(BlockId)
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// The shape a block drawn as boxes takes, made ready for its facing.
    pub fn shape(&self, id: BlockId) -> Option<&crate::shape::Shape> {
        self.info(id).shape.map(|s| &self.shapes[s as usize])
    }

    /// What a block stops movement with, in blocks from its cell's lower corner: nothing for
    /// one walked through, its shape's boxes for a solid block drawn as one (a slab's lower
    /// half, a table's top and legs), else the whole cell. Players move against these, on the
    /// client and as the server judges them.
    pub fn collision(&self, id: BlockId) -> &[crate::physics::Aabb] {
        let info = self.info(id);
        match (info.solid, info.shape) {
            (false, _) => &[],
            (true, Some(s)) => &self.shape_collision[s as usize],
            (true, None) => &crate::physics::CUBE,
        }
    }

    /// What a creature's blow meets of a solid block drawn as boxes at a player standing in
    /// its cell (`Shape::hull`): the span of each run of it, gaps and all; else the whole cell.
    pub fn hull(&self, id: BlockId) -> &[crate::physics::Aabb] {
        let info = self.info(id);
        match (info.solid, info.shape) {
            (false, _) => &[],
            (true, Some(s)) => &self.shape_hull[s as usize],
            (true, None) => &crate::physics::CUBE,
        }
    }

    /// Where a block's shape's quads start in all shapes' quads, in order, as the renderer's
    /// table holds them.
    pub fn shape_first_quad(&self, id: BlockId) -> Option<usize> {
        self.info(id).shape.map(|s| self.shape_starts[s as usize] as usize)
    }

    /// The texture layer of each quad of a block's shape.
    pub fn shape_layers(&self, id: BlockId) -> &[u16] {
        &self.shape_layers[id.0 as usize]
    }

    /// Every shape blocks take, by [`BlockInfo::shape`]: what the renderer's table holds.
    pub fn shapes(&self) -> &[crate::shape::Shape] {
        &self.shapes
    }

    pub fn textures(&self) -> &[TextureRef] {
        &self.textures
    }

    /// Stable fingerprint used to check that client and server load the same content.
    pub fn fingerprint(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for def in &self.defs {
            for b in def.name.bytes().chain([0]) {
                h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"(
        templates: {
            "wood": [
                (name: "{name}_log", textures: Column(end: "{name}_log_top", side: "{name}_log"), tags: ["log"]),
                (name: "{name}_planks", textures: All("{name}_planks"), tags: ["planks"]),
                (name: "{name}_leaves", render: Leaves, tint: Foliage, drops: "{name}_sapling", tags: ["leaves"]),
                (name: "{name}_sapling", render: Plant, solid: false, textures: All("{name}_sapling")),
            ],
        },
        blocks: [
            (name: "stone", textures: All("stone"), tags: ["stone", "igneous"]),
            (
                name: "poppy", render: Plant, solid: false, textures: All("poppy"), tags: ["flower", "dye:red"],
                rarity: Uncommon,
                variants: [(name: "pair", textures: All("poppy_pair"), size: 2), (name: "bunch", textures: All("poppy_bunch"), size: 3, rarity: Rare)],
            ),
            (name: "dirt", drops: Some("stone")),
        ],
        families: [
            (template: "wood", name: "oak", tags: ["broadleaf"]),
            (
                template: "wood", name: "cherry", tags: ["broadleaf"],
                members: {"cherry_leaves": (tint: Some(None), light: 3, tags: ["blossom"])},
                without: ["cherry_planks"],
            ),
        ],
    )"#;

    #[test]
    fn emissions_take_their_defaults() {
        let def: BlockDef = ron::from_str(
            "(name: \"torch\", particles: [(style: Flame, rate: 10.0, at: (0.5, 0.6, 0.5)), (style: Spark, count: 4, color: Some((1, 2, 3)))])",
        )
        .unwrap();
        assert_eq!(def.particles.len(), 2);
        assert_eq!(def.particles[0], Emission { style: ParticleStyle::Flame, rate: 10.0, at: [0.5, 0.6, 0.5], ..Emission::default() });
        assert_eq!((def.particles[1].count, def.particles[1].rate, def.particles[1].color), (4, 1.0, Some([1, 2, 3])));
        assert!(BlockDef::default().particles.is_empty());
    }

    /// A chimney's smoke, seen from afar over a hearth, loads; one seen past the limit, or
    /// over nothing, does not.
    #[test]
    fn far_and_conditional_emissions_are_checked() {
        let block = |emission: &str| format!("(blocks: [(name: \"chimney\", textures: All(\"chimney\"), particles: [{emission}])])");
        let registry = BlockRegistry::from_ron("base", &block("(range: Some(96.0), over: Some((\"hearth\", 3)))")).unwrap();
        let smoke = &registry.def(registry.expect("base:chimney")).particles[0];
        assert_eq!((smoke.range, smoke.over.clone()), (Some(96.0), Some(("hearth".to_string(), 3))));
        assert!(BlockRegistry::from_ron("base", &block("(range: Some(500.0))")).is_err());
        assert!(BlockRegistry::from_ron("base", &block("(over: Some((\"\", 3)))")).is_err());
        assert!(BlockRegistry::from_ron("base", &block("(over: Some((\"hearth\", 0)))")).is_err());
    }

    #[test]
    fn families_expand_in_order() {
        let defs = BlockRegistry::parse("base", BASE).unwrap();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "base:stone", "base:poppy", "base:dirt", "base:oak_log", "base:oak_planks", "base:oak_leaves", "base:oak_sapling",
                "base:cherry_log", "base:cherry_leaves", "base:cherry_sapling",
            ]
        );
        let registry = BlockRegistry::new(defs).unwrap();
        let oak_leaves = registry.def(registry.expect("base:oak_leaves"));
        assert_eq!(oak_leaves.tint, BlockTint::Foliage);
        assert_eq!(oak_leaves.family.as_deref(), Some("base:oak"));
        assert_eq!(oak_leaves.tags, ["leaves", "broadleaf"]);
        assert_eq!(registry.info(registry.expect("base:oak_leaves")).drops, registry.expect("base:oak_sapling"));
        let cherry_leaves = registry.def(registry.expect("base:cherry_leaves"));
        assert_eq!((cherry_leaves.tint, cherry_leaves.light), (BlockTint::None, 3));
        assert_eq!(cherry_leaves.tags, ["leaves", "broadleaf", "blossom"]);
        assert!(matches!(&registry.def(registry.expect("base:cherry_log")).textures, Textures::Column { end, .. } if end == "cherry_log_top"));
    }

    /// A package names the blocks it renamed, and older saves' names find the new block;
    /// a rename to nothing, or of a block that still exists, is refused at load.
    #[test]
    fn renamed_blocks_resolve_for_older_saves_and_are_load_checked() {
        let source = r#"(blocks: [(name: "small_game", item: true)], renamed: {"rabbit": "small_game", "chicken": "base:small_game"})"#;
        let file = BlockRegistry::read("base", source).unwrap();
        let renames = BlockRegistry::renames(&[("base".to_owned(), file)]);
        let mut blocks = BlockRegistry::new(BlockRegistry::parse("base", source).unwrap()).unwrap();
        blocks.rename(renames.clone()).unwrap();
        let game = blocks.expect("base:small_game");
        assert_eq!((blocks.item_named("base:rabbit"), blocks.saved_id("base:chicken")), (Some(game), Some(game)));
        assert_eq!(blocks.id("base:rabbit"), None, "content cannot name it any more");
        let err = |renames: Vec<(String, String)>| BlockRegistry::new(BlockRegistry::parse("base", source).unwrap()).unwrap().rename(renames).unwrap_err().0;
        assert!(err(vec![("base:rabbit".into(), "base:hare".into())]).contains("unknown block `base:hare`"));
        assert!(err(vec![("base:small_game".into(), "base:small_game".into())]).contains("still called"));
        assert!(err([renames.clone(), renames].concat()).contains("renamed twice"));
    }

    #[test]
    fn a_fixed_tint_is_baked_into_its_own_layers() {
        let source = r#"(
            templates: {"cloth": [(name: "{name}_cloth", textures: All("cloth"))]},
            blocks: [(name: "plain_cloth", textures: All("cloth"))],
            families: [
                (template: "cloth", name: "red", all: (tint: Some(Fixed(200, 40, 30)))),
                (template: "cloth", name: "blue", all: (tint: Some(Fixed(30, 60, 190)))),
                (template: "cloth", name: "red_again", all: (tint: Some(Fixed(200, 40, 30)))),
            ],
        )"#;
        let registry = BlockRegistry::from_ron("base", source).unwrap();
        let layer = |name: &str| registry.info(registry.expect(name)).layers[0];
        // One drawing, three colours: a layer each for the two colours, shared by the two
        // blocks that take one, and a plain one.
        assert_ne!(layer("base:red_cloth"), layer("base:blue_cloth"));
        assert_ne!(layer("base:red_cloth"), layer("base:plain_cloth"));
        assert_eq!(layer("base:red_cloth"), layer("base:red_again_cloth"));
        let tinted = |name: &str| registry.textures()[layer(name) as usize].tint;
        assert_eq!(tinted("base:red_cloth"), Some([200, 40, 30]));
        assert_eq!(tinted("base:blue_cloth"), Some([30, 60, 190]));
        assert_eq!(tinted("base:plain_cloth"), None);
        // The renderer is told of no tint at all.
        assert_eq!(registry.info(registry.expect("base:red_cloth")).tints, [Tint::None; 6]);
    }

    #[test]
    fn a_family_over_the_dyes_is_made_once_for_each() {
        let source = r#"(
            dyes: [(name: "red", rgb: (200, 40, 30)), (name: "blue", rgb: (30, 60, 190))],
            templates: {"cloth": [(name: "{name}_cloth", textures: All("cloth")), (name: "{name}_rug", textures: All("cloth"), tags: ["rug"])]},
            families: [(template: "cloth", dyes: true, tags: ["decor"])],
        )"#;
        let registry = BlockRegistry::from_ron("base", source).unwrap();
        let blue = registry.def(registry.expect("base:blue_rug"));
        assert_eq!((blue.tint, blue.color, blue.family.as_deref()), (BlockTint::Fixed(30, 60, 190), [30, 60, 190], Some("base:blue")));
        assert_eq!(blue.tags, ["rug", "decor"]);
        assert_eq!(registry.def(registry.expect("base:red_cloth")).tint, BlockTint::Fixed(200, 40, 30));
        assert_eq!(registry.len(), 5, "air and two members for each of two dyes");
        let twice = r#"(dyes: [(name: "red", rgb: (1, 2, 3)), (name: "red", rgb: (4, 5, 6))])"#;
        assert!(BlockRegistry::from_ron("base", twice).is_err());
        let none = r#"(templates: {"t": [(name: "{name}_t")]}, families: [(template: "t", dyes: true)])"#;
        assert!(BlockRegistry::from_ron("base", none).is_err());
    }

    #[test]
    fn variants_are_blocks_that_drop_their_plant() {
        let registry = BlockRegistry::from_ron("base", BASE).unwrap();
        let poppy = registry.expect("base:poppy");
        let bunch = registry.expect("base:poppy_bunch");
        assert_eq!(registry.id("base:poppy_pair"), Some(BlockId(poppy.0 + 1)));
        assert_eq!((registry.info(bunch).drops, registry.info(bunch).drop_count), (poppy, 3));
        assert_eq!(registry.def(bunch).variant_of.as_deref(), Some("base:poppy"));
        assert_eq!(registry.def(bunch).rarity, Rarity::Uncommon);
        let looks = registry.looks(poppy);
        assert_eq!(looks.len(), 3);
        assert_eq!((looks[0].block, looks[0].size, looks[0].rarity), (poppy, 1, Rarity::Common));
        assert_eq!((looks[2].block, looks[2].size, looks[2].rarity), (bunch, 3, Rarity::Rare));
        assert_eq!(registry.looks(bunch), looks);
        assert_eq!(registry.looks(registry.expect("base:stone")).len(), 1);
        // Variants share tags but are reached through their plant.
        assert_eq!(registry.tagged("dye:red").collect::<Vec<_>>(), [poppy]);
    }

    #[test]
    fn templates_are_shared_across_packages() {
        let base = BlockRegistry::read("base", BASE).unwrap();
        let addon = BlockRegistry::read("addon", r#"(families: [(template: "base:wood", name: "maple")])"#).unwrap();
        // The add-on loads first, yet finds base's template.
        let defs = BlockRegistry::expand(vec![("addon".into(), addon), ("base".into(), base)]).unwrap();
        assert_eq!(defs[0].name, "addon:maple_log");
        assert_eq!(defs[2].drops.as_deref(), Some("addon:maple_sapling"));
        assert!(BlockRegistry::new(defs).is_ok());
    }

    /// Shapes as a package's shapes.ron gives them.
    fn shapes() -> crate::shape::Shapes {
        let file = crate::shape::ShapeFile::read(
            "base",
            r#"(
                shapes: {
                    "slab": (boxes: [(to: (16, 8, 16), cull: true)]),
                    "stairs": (boxes: [(to: (16, 8, 16), cull: true), (from: (0, 8, 8), cull: true)]),
                },
            )"#,
        )
        .unwrap();
        crate::shape::Shapes::gather(vec![("base".into(), file)], crate::shape::ShapeLimits { boxes: 4, quads: 128 }).unwrap()
    }

    #[test]
    fn shaped_blocks_take_their_shapes_and_share_them() {
        let defs = BlockRegistry::parse(
            "base",
            r#"(blocks: [
                (name: "planks", textures: All("planks")),
                (name: "slab", render: Model, shape: "slab", flips: true, textures: All("planks")),
                (name: "birch_slab", render: Model, shape: "slab", textures: All("birch_planks")),
                (name: "stairs", render: Model, shape: "stairs", facing: true, flips: true, textures: All("planks")),
            ])"#,
        )
        .unwrap();
        // Blocks read alone (a test's or a tool's) have no shapes; the game's have theirs.
        let alone = BlockRegistry::new(defs.clone()).unwrap();
        assert!(alone.shape(alone.expect("base:slab")).is_none() && alone.shapes().is_empty());
        let registry = BlockRegistry::with_shapes(defs, &shapes()).unwrap();
        let (slab, birch) = (registry.expect("base:slab"), registry.expect("base:birch_slab"));
        // One shape for both slabs, each in its own texture.
        assert_eq!(registry.info(slab).shape, registry.info(birch).shape);
        assert_ne!(registry.shape_layers(slab), registry.shape_layers(birch));
        // Only its bottom is whole, so it hides no neighbour and lets light through.
        let full = |block| registry.shape(block).unwrap().full;
        assert_eq!(full(slab).map(u8::from), [0, 0, 0, 1, 0, 0]);
        assert!(!registry.info(slab).occludes);
        // Upside down it is a top slab, whole on top, dropping and named as the slab.
        let top = registry.expect("base:slab_top");
        assert_eq!(registry.flipped(slab), Some(top));
        assert_eq!(full(top).map(u8::from), [0, 0, 1, 0, 0, 0]);
        assert_eq!((registry.info(top).drops, registry.def(top).form_of()), (slab, Some("base:slab")));
        // Stairs come in four facings both ways up, each a placement form of the stairs.
        let (stairs, east) = (registry.expect("base:stairs"), registry.expect("base:stairs_east"));
        let east_top = registry.expect("base:stairs_top_east");
        assert_eq!(registry.flipped(east), Some(east_top));
        assert_eq!(registry.flipped(east_top), None);
        assert_eq!((registry.info(east_top).drops, registry.def(east_top).form_of()), (stairs, Some("base:stairs")));
        assert!(registry.placement_form(east_top) && registry.placement_form(top) && !registry.placement_form(stairs));
        // Facing east, the stairs climb westwards: their back is their west side.
        assert_eq!(full(east).map(u8::from), [0, 1, 0, 1, 0, 0]);
        // A shape is kept once for each facing and way up: two slabs, eight stairs.
        assert_eq!(registry.shapes().len(), 10);
    }

    #[test]
    fn shaped_blocks_are_checked_at_load() {
        let refused = |blocks: &str| {
            let defs = BlockRegistry::parse("base", &format!("(blocks: [{blocks}])")).unwrap();
            BlockRegistry::with_shapes(defs, &shapes()).unwrap_err().0
        };
        assert!(refused(r#"(name: "a", render: Model, shape: "nope")"#).contains("unknown shape `base:nope`"));
        assert!(refused(r#"(name: "a", render: Model)"#).contains("names none"));
        assert!(refused(r#"(name: "a", shape: "slab")"#).contains("not drawn as one"));
        assert!(refused(r#"(name: "a", flips: true)"#).contains("upside down"));
        // A torch goes on walls only as a block that rests on a wall, and goes with it.
        assert!(refused(r#"(name: "torch", wall: "lamp"), (name: "lamp", facing: true)"#).contains("does not rest on a wall"));
    }

    /// A window lets light in: one tagged as a window, a lattice, a frame, a slit or a
    /// draughty gap, or lit through, is refused if it is a whole cube or dims light as a
    /// shut shutter does; a slab-thin pane or a lattice of boxes is let through.
    #[test]
    fn a_window_that_keeps_the_light_out_is_refused() {
        let load = |blocks: &str| {
            let defs = BlockRegistry::parse("base", &format!("(blocks: [{blocks}])")).unwrap();
            BlockRegistry::with_shapes(defs, &shapes()).map(|_| ()).map_err(|e| e.0)
        };
        assert_eq!(load(r#"(name: "pane", render: Cutout, tags: ["window"], lit_through: true)"#), Ok(()));
        assert_eq!(load(r#"(name: "lattice", render: Model, shape: "slab", tags: ["lattice", "draught"])"#), Ok(()));
        let whole = load(r#"(name: "pane", tags: ["window"])"#).unwrap_err();
        assert!(whole.contains("`base:pane` is a window (tagged `window`)"), "{whole}");
        let dim = load(r#"(name: "slit", render: Model, shape: "slab", light_filter: 14, tags: ["slit"])"#).unwrap_err();
        assert!(dim.contains("`light_filter` is 2 at most, not 14"), "{dim}");
        assert!(load(r#"(name: "glow", lit_through: true)"#).unwrap_err().contains("(`lit_through`)"));
        assert!(load(r#"(name: "gap", render: Model, shape: "slab", light_filter: 3, tags: ["draught"])"#).is_err());
    }

    /// A window's drawing glows by the light through it, so a block drawn the same, on a
    /// face or as a box of its shape, is refused, whatever window and other block are; a
    /// window drawn its own way, or two windows sharing one, load.
    #[test]
    fn a_window_drawn_as_a_block_that_does_not_glow_is_refused() {
        let load = |blocks: &str| {
            let defs = BlockRegistry::parse("base", &format!("(blocks: [{blocks}])")).unwrap();
            BlockRegistry::with_shapes(defs, &shapes()).map(|_| ()).map_err(|e| e.0)
        };
        let window = r#"(name: "lattice", render: Model, shape: "slab", textures: All("grain"), tags: ["window"], lit_through: true)"#;
        let shared = load(&format!(r#"{window}, (name: "post", render: Model, shape: "stairs", textures: All("grain"))"#)).unwrap_err();
        assert!(shared.contains("`base:post` is drawn as window `base:lattice` is"), "{shared}");
        let sided = load(&format!(r#"{window}, (name: "beam", textures: Sides(top: "oak", bottom: "oak", side: "grain"))"#)).unwrap_err();
        assert!(sided.contains("`base:beam`"), "{sided}");
        assert_eq!(load(&format!(r#"{window}, (name: "post", render: Model, shape: "stairs", textures: All("timber"))"#)), Ok(()));
        assert_eq!(load(&format!(r#"{window}, (name: "pane", render: Cutout, textures: All("grain"), tags: ["window"], lit_through: true)"#)), Ok(()));
        // Faces with no drawing of their own are no sharing (they all count as the first).
        assert_eq!(load(&format!(r#"(name: "stone"), {window}"#)), Ok(()));
    }

    /// A railing that is not solid or not a block high, a landing with no floor across the
    /// top of its cell, or a chimney piece or ladder that is not solid, is refused; a whole
    /// block passes as any.
    #[test]
    fn a_railing_or_landing_that_would_let_someone_fall_is_refused() {
        let load = |blocks: &str| {
            let defs = BlockRegistry::parse("base", &format!("(blocks: [{blocks}])")).unwrap();
            BlockRegistry::with_shapes(defs, &shapes()).map(|_| ()).map_err(|e| e.0)
        };
        assert_eq!(load(r#"(name: "rail", render: Model, shape: "stairs", tags: ["railing"])"#), Ok(()));
        assert_eq!(load(r#"(name: "floor", tags: ["landing"])"#), Ok(()));
        let low = load(r#"(name: "rail", render: Model, shape: "slab", tags: ["railing"])"#).unwrap_err();
        assert!(low.contains("`base:rail` is a railing"), "{low}");
        assert!(load(r#"(name: "rail", render: Model, shape: "stairs", solid: false, tags: ["railing"])"#).is_err());
        let sunk = load(r#"(name: "floor", render: Model, shape: "slab", tags: ["landing"])"#).unwrap_err();
        assert!(sunk.contains("`base:floor` is a landing"), "{sunk}");
        assert!(load(r#"(name: "floor", render: Model, shape: "stairs", tags: ["landing"])"#).is_err());
        assert_eq!(load(r#"(name: "flue", render: Model, shape: "slab", tags: ["chimney_piece"])"#), Ok(()));
        let open = load(r#"(name: "flue", render: Model, shape: "slab", solid: false, tags: ["chimney_piece"])"#).unwrap_err();
        assert!(open.contains("`base:flue` is a chimney piece"), "{open}");
        assert_eq!(load(r#"(name: "ladder", render: Model, shape: "slab", tags: ["climbable"])"#), Ok(()));
        let loose = load(r#"(name: "ladder", render: Model, shape: "slab", solid: false, tags: ["climbable"])"#).unwrap_err();
        assert!(loose.contains("`base:ladder` is climbable"), "{loose}");
    }

    #[test]
    fn reports_mistakes_in_families() {
        let typo = r#"(templates: {"t": [(name: "{name}_a")]}, families: [(template: "t", name: "x", members: {"x_b": (light: 1)})])"#;
        assert!(BlockRegistry::parse("base", typo).unwrap_err().0.contains("x_b"));
        let missing = r#"(families: [(template: "nope", name: "x")])"#;
        assert!(BlockRegistry::parse("base", missing).is_err());
    }
}
