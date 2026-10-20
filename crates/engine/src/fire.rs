//! Fire in the thatch, as content (`fire.ron`): what catches fire from a flame beside it, how
//! long it takes, what burning is and what is left.
//!
//! A block with one of the `catches` tags (thatch, wattle) that a flame stands within `reach`
//! of for `catch_seconds` catches: it becomes the `burning` block, itself a flame, so the fire
//! runs on through what catches beside it, and after `burn_seconds` it is gone (`ash`). A
//! flame is a block with one of the `flames` tags (a torch, a campfire, a brazier, burning
//! thatch) or a lit hearth, which reaches `hearth_reach`. There is no chance in it: a torch
//! set against a thatch roof always sets it alight, and taking the torch away in time, or
//! breaking the burning block, puts it out. While it waits to catch it smoulders, smoking
//! (`smoke`) and crackling (ambience.ron), so a player sees it coming.

use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FireFile {
    /// A block with any of these tags catches fire.
    pub catches: Vec<String>,
    /// A block with any of these tags is a flame.
    pub flames: Vec<String>,
    /// How far a flame reaches, every way (1 to 4): a block within a cube this far around it.
    pub reach: i32,
    /// How far a lit hearth reaches (0 to 4; 0 for none).
    pub hearth_reach: i32,
    /// Seconds a flame must stand within reach for a block to catch.
    pub catch_seconds: f32,
    /// Seconds a block burns before it is gone.
    pub burn_seconds: f32,
    /// The block that is burning.
    pub burning: String,
    /// What is left where it burned.
    pub ash: String,
    /// What a block waiting to catch gives off, drawn over it: the smoke that says it is
    /// about to go up (the crackle is ambience.ron's `smoulder` group).
    #[serde(default)]
    pub smoke: Vec<crate::block::Emission>,
    /// A block inside a player's home box (their lit home hearth's reach, or where their bed
    /// says home is) only smoulders or catches from a fire that player started, so nobody can
    /// burn down someone else's house with a torch at the wall. On unless a pack turns it off.
    #[serde(default = "yes")]
    pub homes_catch_only_from_owner: bool,
}

fn yes() -> bool {
    true
}

/// The fire rules, with what each block is to them looked up once.
#[derive(Clone, Debug, Default)]
pub struct Fire {
    pub rules: FireFile,
    pub burning: Option<BlockId>,
    pub ash: Option<BlockId>,
    /// By block id: whether it catches, and whether it is a flame.
    catches: Vec<bool>,
    flames: Vec<bool>,
}

impl Fire {
    pub fn read(namespace: &str, source: &str) -> Result<FireFile, RegistryError> {
        let mut file: FireFile = ron::from_str(source).map_err(|e| RegistryError(format!("{namespace}/fire.ron: {e}")))?;
        for name in [&mut file.burning, &mut file.ash] {
            if !name.is_empty() && !name.contains(':') {
                *name = format!("{namespace}:{name}");
            }
        }
        Ok(file)
    }

    /// The last package's rules win, with tags gathered from all of them. No file, no fire.
    pub fn new(files: Vec<FireFile>, blocks: &BlockRegistry) -> Result<Self, RegistryError> {
        let (mut catches, mut flames) = (Vec::new(), Vec::new());
        let mut rules = None;
        for file in files {
            catches.extend(file.catches.iter().cloned());
            flames.extend(file.flames.iter().cloned());
            rules = Some(file);
        }
        let Some(mut rules) = rules else { return Ok(Self::default()) };
        (rules.catches, rules.flames) = (catches, flames);
        if !(1..=4).contains(&rules.reach) || !(0..=4).contains(&rules.hearth_reach) {
            return Err(RegistryError(format!("fire.ron: a flame reaches 1 to 4 blocks and a hearth 0 to 4, not {} and {}", rules.reach, rules.hearth_reach)));
        }
        if rules.smoke.is_empty() {
            return Err(RegistryError("fire.ron: a block about to catch gives off `smoke`, so a player sees it coming".into()));
        }
        if !(rules.catch_seconds > 0.0 && rules.burn_seconds > 0.0) {
            return Err(RegistryError(format!("fire.ron: catching takes {} s and burning {} s; both take some", rules.catch_seconds, rules.burn_seconds)));
        }
        let id = |name: &str| blocks.id(name).ok_or_else(|| RegistryError(format!("fire.ron: no block `{name}`")));
        let (burning, ash) = (id(&rules.burning)?, id(&rules.ash)?);
        let has = |block: BlockId, tags: &[String]| tags.iter().any(|t| blocks.def(block).has_tag(t));
        if !has(burning, &rules.flames) {
            return Err(RegistryError(format!("fire.ron: the burning block `{}` is no flame (none of {:?})", rules.burning, rules.flames)));
        }
        if has(burning, &rules.catches) || has(ash, &rules.catches) {
            return Err(RegistryError("fire.ron: what burns or is left after cannot catch again".into()));
        }
        let all: Vec<BlockId> = blocks.ids().collect();
        let size = all.iter().map(|b| b.0 as usize + 1).max().unwrap_or(0);
        let (mut catch, mut flame) = (vec![false; size], vec![false; size]);
        for block in all {
            catch[block.0 as usize] = has(block, &rules.catches);
            flame[block.0 as usize] = has(block, &rules.flames);
        }
        if let Some(both) = (0..size).find(|&i| catch[i] && flame[i]) {
            return Err(RegistryError(format!("fire.ron: `{}` both catches and is a flame", blocks.def(BlockId(both as _)).name)));
        }
        Ok(Self { rules, burning: Some(burning), ash: Some(ash), catches: catch, flames: flame })
    }

    /// Whether `block` catches fire.
    pub fn catches(&self, block: BlockId) -> bool {
        self.catches.get(block.0 as usize).copied().unwrap_or(false)
    }

    /// Whether `block` is a flame.
    pub fn flame(&self, block: BlockId) -> bool {
        self.flames.get(block.0 as usize).copied().unwrap_or(false)
    }

    /// Whether fire does anything at all.
    pub fn on(&self) -> bool {
        self.burning.is_some()
    }
}
