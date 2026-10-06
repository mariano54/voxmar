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

#[derive(Clone, Debug, Serialize, Deserialize)]
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

    fn face(&self, face: Face) -> Option<&str> {
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

/// A block definition as authored in content files.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BlockDef {
    pub name: String,
    pub render: RenderKind,
    /// Whether entities collide with the block.
    pub solid: bool,
    pub textures: Textures,
    pub tint: Tint,
    /// Only tint the top face (grass blocks).
    pub tint_top_only: bool,
    /// Light level emitted, 0..=15.
    pub light: u8,
    /// Extra light attenuation for non-opaque blocks (water, leaves).
    pub light_filter: u8,
    /// Seconds to break by hand. Negative means unbreakable.
    pub hardness: f32,
    pub sound: Sound,
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
}

impl Default for Emission {
    fn default() -> Self {
        Self { style: ParticleStyle::Smoke, rate: 1.0, count: 1, at: [0.5, 0.5, 0.5], spread: 0.1, color: None, size: 1.0 }
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
    /// Two quads crossing on the cell's diagonals: grass and most flowers.
    #[default]
    Cross,
    /// Four quads in a #, straight with the blocks: crops and reeds, as Minecraft's wheat.
    Hash,
    /// A crossed stem under a flat cap of the block's top texture: mushrooms.
    Cap,
    /// A cross with a second, straight one through it: fuller bushes and shrubs.
    Bush,
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
}

impl Default for BlockDef {
    fn default() -> Self {
        Self {
            name: String::new(),
            render: RenderKind::Opaque,
            solid: true,
            textures: Textures::None,
            tint: Tint::None,
            tint_top_only: false,
            light: 0,
            light_filter: 0,
            hardness: 1.0,
            sound: Sound::Stone,
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
            model: PlantModel::Cross,
            plant_height: 1.0,
            host: None,
            particles: Vec::new(),
        }
    }
}

impl BlockDef {
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// Replaces `{name}` in every name the definition holds.
    fn substitute(&mut self, name: &str) {
        let fill = |s: &mut String| *s = s.replace("{name}", name);
        fill(&mut self.name);
        self.textures.names_mut().into_iter().for_each(fill);
        self.drops.iter_mut().for_each(fill);
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
}

impl Default for Variant {
    fn default() -> Self {
        Self { name: String::new(), textures: Textures::None, size: 1, rarity: Rarity::Common }
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
    pub tint: Option<Tint>,
    pub tint_top_only: Option<bool>,
    pub light: Option<u8>,
    pub light_filter: Option<u8>,
    pub hardness: Option<f32>,
    pub sound: Option<Sound>,
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
}

#[derive(Debug)]
pub struct BlockRegistry {
    defs: Vec<BlockDef>,
    info: Vec<BlockInfo>,
    by_name: FxHashMap<String, BlockId>,
    textures: Vec<TextureRef>,
    /// Every block's looks: itself and its variants (shared by the variants).
    looks: Vec<Vec<Look>>,
    fluids: Vec<Fluid>,
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
                let mut names = Vec::new();
                for member in template {
                    let mut def = member.clone();
                    def.substitute(&family.name);
                    names.push(def.name.clone());
                    if family.without.contains(&def.name) {
                        continue;
                    }
                    def.tags.extend(family.tags.iter().cloned());
                    family.all.apply(&mut def);
                    if let Some(patch) = family.members.get(&def.name) {
                        patch.apply(&mut def);
                    }
                    def.family = Some(family.name.clone());
                    def.qualify(&namespace);
                    defs.push(def);
                }
                if let Some(unknown) = family.members.keys().chain(&family.without).find(|n| !names.contains(n)) {
                    return Err(error(format!("`{unknown}` is not one of its members ({})", names.join(", "))));
                }
            }
        }
        Ok(defs)
    }

    /// Builds the registry from fully qualified definitions, making each variant its own
    /// block right after the block it varies. `core:air` is always id 0.
    pub fn new(content: Vec<BlockDef>) -> Result<Self, RegistryError> {
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
            defs.push(def);
            for variant in &base.variants {
                defs.push(BlockDef {
                    name: format!("{}_{}", base.name, variant.name),
                    textures: variant.textures.clone(),
                    drops: Some(base.drops.clone().unwrap_or_else(|| base.name.clone())),
                    drop_count: variant.size,
                    variants: Vec::new(),
                    variant_of: Some(base.name.clone()),
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
        let mut texture_index = FxHashMap::<(String, Option<String>), u16>::default();
        let mut info = Vec::with_capacity(defs.len());
        for (def, &fluid) in defs.iter().zip(&states) {
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
                    layers[face as usize] = *texture_index.entry((name.to_owned(), under.clone())).or_insert_with(|| {
                        textures.push(TextureRef { name: name.to_owned(), under, color: def.color, render: def.render });
                        (textures.len() - 1) as u16
                    });
                }
                if !def.tint_top_only || face == Face::PosY {
                    tints[face as usize] = def.tint;
                }
            }
            let drops = match &def.drops {
                Some(name) => *by_name
                    .get(name)
                    .ok_or_else(|| RegistryError(format!("`{}` drops unknown block `{name}`", def.name)))?,
                None => by_name[&def.name],
            };
            let occludes = def.render == RenderKind::Opaque;
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

        Ok(Self { defs, info, by_name, textures, looks, fluids })
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

    /// Like [`Self::id`] but for names the engine or generator cannot work without.
    pub fn expect(&self, name: &str) -> BlockId {
        self.id(name).unwrap_or_else(|| panic!("content is missing required block `{name}`"))
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
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
        assert_eq!(oak_leaves.tint, Tint::Foliage);
        assert_eq!(oak_leaves.family.as_deref(), Some("base:oak"));
        assert_eq!(oak_leaves.tags, ["leaves", "broadleaf"]);
        assert_eq!(registry.info(registry.expect("base:oak_leaves")).drops, registry.expect("base:oak_sapling"));
        let cherry_leaves = registry.def(registry.expect("base:cherry_leaves"));
        assert_eq!((cherry_leaves.tint, cherry_leaves.light), (Tint::None, 3));
        assert_eq!(cherry_leaves.tags, ["leaves", "broadleaf", "blossom"]);
        assert!(matches!(&registry.def(registry.expect("base:cherry_log")).textures, Textures::Column { end, .. } if end == "cherry_log_top"));
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

    #[test]
    fn reports_mistakes_in_families() {
        let typo = r#"(templates: {"t": [(name: "{name}_a")]}, families: [(template: "t", name: "x", members: {"x_b": (light: 1)})])"#;
        assert!(BlockRegistry::parse("base", typo).unwrap_err().0.contains("x_b"));
        let missing = r#"(families: [(template: "nope", name: "x")])"#;
        assert!(BlockRegistry::parse("base", missing).is_err());
    }
}
