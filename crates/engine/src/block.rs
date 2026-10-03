//! Block definitions and the registry that compiles them into lookup tables.
//!
//! Definitions are plain data (RON). First-party content goes through exactly the same
//! loading path a mod's content will.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::coords::Face;

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
    /// Alpha-tested cube (leaves). Drawn in the opaque pass, does not hide neighbours.
    Cutout,
    /// Alpha-blended cube (water). Drawn in the translucent pass.
    Translucent,
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
}

impl Sound {
    pub const ALL: [Sound; 10] = [
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
    /// Name of the block dropped when broken. Defaults to the block itself.
    pub drops: Option<String>,
    /// Placeholder texture colour used when no texture pack is available.
    pub color: [u8; 3],
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
            color: [255, 0, 255],
        }
    }
}

#[derive(Debug, Deserialize)]
struct BlockFile {
    blocks: Vec<BlockDef>,
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
    pub drops: BlockId,
}

/// A texture referenced by block definitions; its index is the texture array layer.
#[derive(Clone, Debug)]
pub struct TextureRef {
    pub name: String,
    pub color: [u8; 3],
}

#[derive(Debug)]
pub struct BlockRegistry {
    defs: Vec<BlockDef>,
    info: Vec<BlockInfo>,
    by_name: FxHashMap<String, BlockId>,
    textures: Vec<TextureRef>,
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

    /// Parses a package's block definitions, qualifying names with its namespace.
    pub fn parse(namespace: &str, source: &str) -> Result<Vec<BlockDef>, RegistryError> {
        let file: BlockFile = ron::from_str(source).map_err(|e| RegistryError(format!("{namespace}/blocks.ron: {e}")))?;
        let qualify = |name: &str| if name.contains(':') { name.to_owned() } else { format!("{namespace}:{name}") };
        Ok(file
            .blocks
            .into_iter()
            .map(|def| BlockDef { name: qualify(&def.name), drops: def.drops.as_deref().map(qualify), ..def })
            .collect())
    }

    /// Builds the registry from fully qualified definitions. `core:air` is always id 0.
    pub fn new(content: Vec<BlockDef>) -> Result<Self, RegistryError> {
        let air = BlockDef {
            name: Self::AIR.into(),
            render: RenderKind::Invisible,
            solid: false,
            hardness: -1.0,
            ..Default::default()
        };
        let defs: Vec<BlockDef> = std::iter::once(air).chain(content).collect();
        if defs.len() > u16::MAX as usize {
            return Err(RegistryError("too many blocks".into()));
        }

        let mut by_name = FxHashMap::default();
        for (i, def) in defs.iter().enumerate() {
            if by_name.insert(def.name.clone(), BlockId(i as u16)).is_some() {
                return Err(RegistryError(format!("duplicate block `{}`", def.name)));
            }
        }

        let mut textures: Vec<TextureRef> = Vec::new();
        let mut texture_index = FxHashMap::<String, u16>::default();
        let mut info = Vec::with_capacity(defs.len());
        for def in &defs {
            let mut layers = [0u16; 6];
            let mut tints = [Tint::None; 6];
            for face in Face::ALL {
                if let Some(name) = def.textures.face(face) {
                    layers[face as usize] = *texture_index.entry(name.to_owned()).or_insert_with(|| {
                        textures.push(TextureRef { name: name.to_owned(), color: def.color });
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
            });
        }
        if textures.len() > 4096 {
            return Err(RegistryError("too many textures (max 4096)".into()));
        }

        Ok(Self { defs, info, by_name, textures })
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
