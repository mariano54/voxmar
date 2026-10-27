//! Tools: what a held item does to breaking blocks, as content (each package's `tools.ron`).
//!
//! A tool is an item tagged `tool`, with its kind's name (`pickaxe`), a `tier:N`, a
//! `speed:X` and maybe a `yield:Y`. Each kind `mines` some block tags: over a block with
//! one, the tool breaks it `speed` times as fast as a bare hand, and its `yield` adds to the
//! chance of an extra drop (and of rarer harvests, for a hoe over crops). Block rules say
//! what breaking a block takes and teaches: a level in a skill, a tool of a kind and tier,
//! and the experience it gives, RuneScape's mining crossed with Minecraft's tool tiers.
//! Better tools make better finds and quicker work; nothing makes the world grow faster.
//!
//! Tools and weapons wear out. An item with `uses:N` lasts that many uses (`Wear` says
//! what a use is: a block it suits, more on one it does not, a blow that lands), and the
//! skill its kind trains saves a use now and then. Worn to nothing it breaks, leaving its
//! `head:ITEM` if it names one, and `mend:ITEM` (or `mend:#TAG`) says what mends it at the
//! block `Wear::mend_near` names (or its own `mend_near:TAG`): back by `Wear::mend` of its
//! uses, never for free.
//!
//! What burns as it is held in hand (a firebrand) says `burns:N`: it lasts N seconds, counted in
//! its stack's wear as a tool's uses are, and leaves `burns_to:ITEM` if it names one. A
//! thing either wears or burns. `held_light:N` lights what is around the hand holding it.

use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::skill::{SkillId, SkillRegistry};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KindDef {
    pub name: String,
    /// Block tags it is made for.
    pub mines: Vec<String>,
    /// The skill whose level saves this kind's wear now and then (`Wear::save_per_level`).
    #[serde(default)]
    pub skill: Option<String>,
}

/// How tools and weapons wear (each package's `tools.ron` `wear`; a later package's
/// replaces an earlier one's).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WearDef {
    /// Uses a break takes from a tool made for the block, and from one that is not.
    pub suited: u16,
    pub unsuited: u16,
    /// Uses a blow that lands takes from what struck it.
    pub hit: u16,
    /// Uses tilling takes from a hoe.
    pub till: u16,
    /// The chance, per level in the skill a kind names, that a use takes no wear.
    pub save_per_level: f32,
    /// The skill that saves a weapon's wear.
    pub weapon_skill: String,
    /// The share of its uses left at which a tool warns it is nearly worn out.
    pub warn: f32,
    /// The fewest uses left a tool warns at, whatever its share (a first tool of few uses
    /// warns with enough left to take it back and mend it); 0 for none.
    #[serde(default)]
    pub warn_uses: u16,
    /// The share of its uses mending gives back.
    pub mend: f32,
    /// The tag of the block mending is done by, unless an item names its own (`mend_near:TAG`).
    pub mend_near: String,
    /// Whether a tool broken in the hand brings up a spare of its kind from the pack (a
    /// weapon, the same weapon); none unless a package says so.
    #[serde(default)]
    pub spare: bool,
}

/// [`WearDef`], resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct Wear {
    pub suited: u16,
    pub unsuited: u16,
    pub hit: u16,
    pub till: u16,
    pub save_per_level: f32,
    pub weapon_skill: Option<SkillId>,
    pub warn: f32,
    pub warn_uses: u16,
    pub mend: f32,
    pub mend_near: String,
    pub spare: bool,
}

impl Wear {
    /// The uses left at which a tool of `uses` warns: `warn` of them, or `warn_uses` if
    /// more, short of the whole.
    pub fn warn_at(&self, uses: u16) -> u16 {
        ((uses as f32 * self.warn).ceil() as u16).max(self.warn_uses).min(uses.saturating_sub(1))
    }
}

impl Default for Wear {
    fn default() -> Self {
        Self { suited: 1, unsuited: 2, hit: 1, till: 1, save_per_level: 0.0, weapon_skill: None, warn: 0.1, warn_uses: 0, mend: 0.5, mend_near: String::new(), spare: false }
    }
}

/// What one use did to a worn item ([`wear_down`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Worn {
    /// Worn a little, or not at all.
    Fine,
    /// Worn down past the uses left where it warns (`Wear::warn_at`), this use.
    Warned,
    /// Worn out: it broke.
    Broke,
}

/// Wears `stack` by `by` uses of the `uses` it has in all (0: it never wears), saying
/// whether this use took it past the warning or broke it. A broken stack is left as it was;
/// what it becomes is the caller's (`ToolRegistry::head`).
pub fn wear_down(stack: &mut crate::inventory::ItemStack, by: u16, uses: u16, warn_at: u16) -> Worn {
    if uses == 0 || by == 0 {
        return Worn::Fine;
    }
    let left = |wear: u16| uses.saturating_sub(wear);
    let before = left(stack.wear);
    stack.wear = stack.wear.saturating_add(by).min(uses);
    let after = left(stack.wear);
    if after == 0 {
        Worn::Broke
    } else if before > warn_at && after <= warn_at {
        Worn::Warned
    } else {
        Worn::Fine
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDef {
    /// The block has all of these.
    pub tags: Vec<String>,
    #[serde(default)]
    pub skill: Option<String>,
    #[serde(default = "first_level")]
    pub level: u8,
    /// The kind of tool it needs, and how good a one.
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub tier: u8,
    /// Experience breaking one gives, in the skill.
    #[serde(default)]
    pub xp: f32,
    /// Sometimes something else instead of its own drop: gravel gives flint one time in
    /// eight (item name, chance).
    #[serde(default)]
    pub instead: Option<(String, f32)>,
    /// The kind of tool `instead` waits on: tall grass gives twine only to a knife, and a
    /// better one of the kind finds it more often (its `yield` adds to the chance).
    #[serde(default)]
    pub by: Option<String>,
    /// For a sample of ore the world laid (`ore_sample`): experience, in the skill, the
    /// first one a player breaks over each vein teaches besides `xp` (prospecting).
    #[serde(default)]
    pub prospect: f32,
}

/// The farthest a tool reaps round a crop (`reap:N`): 2 is a five by five patch.
pub const MAX_REAP: u8 = 2;

fn first_level() -> u8 {
    1
}

/// One package's `tools.ron`.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ToolFile {
    #[serde(default)]
    pub kinds: Vec<KindDef>,
    #[serde(default)]
    pub blocks: Vec<RuleDef>,
    #[serde(default)]
    pub wear: Option<WearDef>,
    /// Seconds the hand waits after each block it breaks before it starts on the next
    /// (Minecraft's five ticks); the last package to give one decides, `DEFAULT_BREAK_WAIT`
    /// if none does.
    #[serde(default)]
    pub break_wait: Option<f32>,
    /// How many times as fast a tool carried must break a block than the one it is broken
    /// with for the player to be told so, once a kind of tool each time they join; 0 never
    /// tells, and 1 tells of any tool faster at all (one only as fast is never told of). The
    /// last package to give one decides, `DEFAULT_FASTER_CARRIED` if none does.
    #[serde(default)]
    pub faster_carried: Option<f32>,
}

/// What breaking a block takes and teaches.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub tags: Vec<String>,
    pub skill: Option<SkillId>,
    pub level: u8,
    pub tool: Option<usize>,
    pub tier: u8,
    pub xp: f32,
    pub instead: Option<(BlockId, f32)>,
    /// The kind of tool `instead` waits on, by index.
    pub by: Option<usize>,
    /// Prospecting experience, for the first sample broken over each vein.
    pub prospect: f32,
}

/// What a held item is as a tool.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tool {
    /// Its kind, by index, if it is a tool at all.
    pub kind: Option<usize>,
    pub tier: u8,
    pub speed: f32,
    pub yield_: f32,
    /// How far round a ripe crop it cuts the ripe crops of the same kind with it
    /// (`reap:N`: 1 is the eight about it), for a tool made for crops.
    pub reap: u8,
    /// Uses it lasts (`uses:N`); 0 if it never wears.
    pub uses: u16,
    /// Seconds it burns, held, before it is spent (`burns:N`); 0 if it never burns.
    pub burns: u16,
    /// How much light it gives held (`held_light:N`, 1 to 15); 0 for none.
    pub held_light: u8,
}

/// Every kind of tool and every block rule, with each item's tool worked out once.
#[derive(Debug, Default)]
pub struct ToolRegistry {
    kinds: Vec<KindDef>,
    rules: Vec<Rule>,
    /// Each block id's tool, and the rule that covers it.
    tools: Vec<Tool>,
    covered: Vec<Option<usize>>,
    /// Each block id: the kinds made for it.
    suited: Vec<Vec<usize>>,
    wear: Wear,
    /// Seconds the hand waits after a break (`ToolFile::break_wait`).
    break_wait: f32,
    /// How much faster a carried tool tells (`ToolFile::faster_carried`); 0 never.
    faster_carried: f32,
    /// Each kind's skill that saves its wear.
    savers: Vec<Option<SkillId>>,
    /// Each item: what is left of it when it breaks, what mends it, and the tag of the
    /// block it is mended by.
    heads: Vec<Option<BlockId>>,
    mends: Vec<Vec<BlockId>>,
    mended_near: Vec<Option<String>>,
    /// Each item that wears: the share of its uses one mend brings back, and the skill and
    /// level its recipe asks, which mending asks too (`ToolRegistry::link`).
    mend_share: Vec<f32>,
    mend_skill: Vec<Option<(SkillId, u8)>>,
    /// Each item whose recipe is firm: mending it takes no boost or schematic either.
    mend_firm: Vec<bool>,
    /// Each item mended by any item with a tag (`mend:#TAG`): the first such tag, for
    /// naming what mends it (`item.mend.tag.TAG`).
    mend_by_tag: Vec<Option<String>>,
    /// Each item that burns: what it leaves once burnt out (`burns_to:ITEM`), if anything.
    burnt: Vec<Option<BlockId>>,
    /// Each tool: what it breaks that a lesser one of its kind cannot (`ToolRegistry::opens`).
    opens: Vec<Vec<BlockId>>,
}

/// Each item's [`ToolRegistry::opens`], found once at load: for a tool, one block for each
/// rule asking for its kind at a tier above the next lower tier any tool of its kind has and
/// up to its own (so a rule at a tier no tool has shows on the first tool past it), taking
/// the plainest block (shortest name, then fewest tags: plain ore stands for its deepslate
/// form and its sample), in
/// the order of the level each asks. Nothing for the lowest tier of a kind, which opens
/// nothing a lesser one kept shut, nor for a tool another of its kind and tier ranks below
/// (slower, or as fast and listed first: a bronze pickaxe opens nothing copper did not).
fn opened(blocks: &BlockRegistry, rules: &[Rule], tools: &[Tool], covered: &[Option<usize>]) -> Vec<Vec<BlockId>> {
    let mut plainest: Vec<Option<BlockId>> = vec![None; rules.len()];
    for (b, rule) in covered.iter().enumerate() {
        if let Some(i) = *rule {
            let id = BlockId(b as u16);
            let plain = |b: BlockId| (blocks.def(b).name.len(), blocks.def(b).tags.len());
            if plainest[i].is_none_or(|f| plain(id) < plain(f)) {
                plainest[i] = Some(id);
            }
        }
    }
    tools
        .iter()
        .enumerate()
        .map(|(i, tool)| {
            let Some(kind) = tool.kind else { return Vec::new() };
            let same = || tools.iter().enumerate().filter(|(_, t)| t.kind == Some(kind));
            let Some(lower) = same().map(|(_, t)| t.tier).filter(|&t| t < tool.tier).max() else { return Vec::new() };
            if same().any(|(j, t)| t.tier == tool.tier && (t.speed < tool.speed || (t.speed == tool.speed && j < i))) {
                return Vec::new();
            }
            let mut shown: Vec<(u8, usize, BlockId)> = rules
                .iter()
                .enumerate()
                .filter(|(_, r)| r.tool == Some(kind) && r.tier > lower && r.tier <= tool.tier)
                .filter_map(|(r, rule)| plainest[r].map(|b| (rule.level, r, b)))
                .collect();
            shown.sort();
            shown.into_iter().map(|(_, _, b)| b).collect()
        })
        .collect()
}

/// How many times as fast a carried tool must break a block to be told of where no package
/// says.
pub const DEFAULT_FASTER_CARRIED: f32 = 2.0;

/// Seconds the hand waits after a break where no package says (Minecraft's five ticks).
pub const DEFAULT_BREAK_WAIT: f32 = 0.25;

/// The tag of a tool that never wears (a pail, a fishing rod): every other tool says
/// `uses:N`.
pub const NEVER_WEARS: &str = "never_wears";

/// What [`ToolRegistry::link`] finds for each item: its mend share, the skill and level
/// mending it asks, and whether that level is firm.
pub type MendTerms = (Vec<f32>, Vec<Option<(SkillId, u8)>>, Vec<bool>);

impl ToolRegistry {
    pub fn read(namespace: &str, source: &str) -> Result<ToolFile, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/tools.ron: {e}")))
    }

    pub fn new(files: Vec<ToolFile>, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        let (mut kinds, mut defs, mut wear_def, mut break_wait, mut faster_carried) = (Vec::new(), Vec::new(), None, None, None);
        for file in files {
            kinds.extend(file.kinds);
            defs.extend(file.blocks);
            wear_def = file.wear.or(wear_def);
            break_wait = file.break_wait.or(break_wait);
            faster_carried = file.faster_carried.or(faster_carried);
        }
        let faster_carried = faster_carried.unwrap_or(DEFAULT_FASTER_CARRIED);
        if !(faster_carried == 0.0 || (1.0..=100.0).contains(&faster_carried)) {
            return Err(RegistryError(format!("tools.ron: `faster_carried` {faster_carried} is neither 0 (never told) nor from 1 to 100 times")));
        }
        let break_wait = break_wait.unwrap_or(DEFAULT_BREAK_WAIT);
        if !(0.0..=2.0).contains(&break_wait) {
            return Err(RegistryError(format!("tools.ron: `break_wait` {break_wait} is not from 0 to 2 seconds")));
        }
        let skill_named = |name: &str| skills.id(name).ok_or_else(|| RegistryError(format!("tools.ron: no skill `{name}`")));
        let wear = match wear_def {
            None => Wear::default(),
            Some(def) => {
                if !(0.0..=1.0).contains(&def.warn) || !(0.0..=1.0).contains(&def.mend) || !(0.0..=0.01).contains(&def.save_per_level) {
                    return Err(RegistryError("tools.ron: wear's `warn` and `mend` are shares from 0 to 1, and `save_per_level` at most 0.01".into()));
                }
                if blocks.ids().all(|id| !blocks.def(id).has_tag(&def.mend_near)) {
                    return Err(RegistryError(format!("tools.ron: nothing has the tag `{}` wear's mending is done by", def.mend_near)));
                }
                Wear { suited: def.suited, unsuited: def.unsuited, hit: def.hit, till: def.till, save_per_level: def.save_per_level, weapon_skill: Some(skill_named(&def.weapon_skill)?), warn: def.warn, warn_uses: def.warn_uses, mend: def.mend, mend_near: def.mend_near, spare: def.spare }
            }
        };
        let savers = kinds.iter().map(|k| k.skill.as_deref().map(skill_named).transpose()).collect::<Result<Vec<_>, _>>()?;
        let kind_of = |name: &str| kinds.iter().position(|k: &KindDef| k.name == name);
        let rules = defs
            .into_iter()
            .map(|def| {
                let skill = match &def.skill {
                    Some(name) => Some(skills.id(name).ok_or_else(|| RegistryError(format!("tools.ron: no skill `{name}`")))?),
                    None => None,
                };
                let tool = match &def.tool {
                    Some(name) => Some(kind_of(name).ok_or_else(|| RegistryError(format!("tools.ron: no kind of tool `{name}`")))?),
                    None => None,
                };
                let instead = match &def.instead {
                    Some((name, chance)) => {
                        let name = if name.contains(':') { name.clone() } else { format!("base:{name}") };
                        Some((blocks.id(&name).ok_or_else(|| RegistryError(format!("tools.ron: no item `{name}`")))?, *chance))
                    }
                    None => None,
                };
                let by = match &def.by {
                    Some(_) if instead.is_none() => return Err(RegistryError(format!("tools.ron: {:?} names a tool for `by` but nothing `instead`", def.tags))),
                    Some(name) => Some(kind_of(name).ok_or_else(|| RegistryError(format!("tools.ron: no kind of tool `{name}`")))?),
                    None => None,
                };
                if !(def.prospect.is_finite() && def.prospect >= 0.0) || (def.prospect > 0.0 && skill.is_none()) {
                    return Err(RegistryError(format!("tools.ron: {:?} prospects for {} experience: it must be a finite amount, not below nothing, in a skill", def.tags, def.prospect)));
                }
                Ok(Rule { tags: def.tags, skill, level: def.level, tool, tier: def.tier, xp: def.xp, instead, by, prospect: def.prospect })
            })
            .collect::<Result<Vec<_>, RegistryError>>()?;
        let number = |tags: &[String], prefix: &str| tags.iter().find_map(|t| t.strip_prefix(prefix)?.parse::<f32>().ok());
        let mut tools = Vec::with_capacity(blocks.len());
        let mut covered = Vec::with_capacity(blocks.len());
        let mut suited = Vec::with_capacity(blocks.len());
        let (mut heads, mut mends, mut mended_near) = (Vec::with_capacity(blocks.len()), Vec::with_capacity(blocks.len()), Vec::with_capacity(blocks.len()));
        let mut mend_by_tag = Vec::with_capacity(blocks.len());
        let mut burnt = Vec::with_capacity(blocks.len());
        for id in blocks.ids() {
            let def = blocks.def(id);
            let tags = &def.tags;
            let bad = |why: String| RegistryError(format!("`{}`: {why}", def.name));
            for once in ["uses:", "head:", "mend_near:", "burns:", "burns_to:", "held_light:"] {
                if tags.iter().filter(|t| t.starts_with(once)).count() > 1 {
                    return Err(bad(format!("more than one `{once}` tag")));
                }
            }
            let uses = match tags.iter().find_map(|t| t.strip_prefix("uses:")) {
                None => 0,
                Some(n) => n.parse::<u16>().ok().filter(|&n| n > 0).ok_or_else(|| bad(format!("`uses:{n}` is not a count above 0")))?,
            };
            // A name without a package is in the item's own.
            let package = def.name.split_once(':').map_or("base", |(package, _)| package);
            let item = |name: &str| {
                let name = if name.contains(':') { name.to_owned() } else { format!("{package}:{name}") };
                blocks.id(&name).ok_or_else(|| bad(format!("no item `{name}`")))
            };
            let head = tags.iter().find_map(|t| t.strip_prefix("head:")).map(item).transpose()?;
            let mut mend = Vec::new();
            mend_by_tag.push(tags.iter().find_map(|t| t.strip_prefix("mend:#")).map(str::to_owned));
            for by in tags.iter().filter_map(|t| t.strip_prefix("mend:")) {
                match by.strip_prefix('#') {
                    Some(tag) => {
                        let tagged: Vec<BlockId> = blocks.ids().filter(|&b| blocks.def(b).has_tag(tag) && blocks.item_of(b) == b).collect();
                        if tagged.is_empty() {
                            return Err(bad(format!("nothing has the tag `{tag}` it is mended with")));
                        }
                        mend.extend(tagged);
                    }
                    None => mend.push(item(by)?),
                }
            }
            let near = tags.iter().find_map(|t| t.strip_prefix("mend_near:")).map(str::to_owned);
            if let Some(tag) = &near
                && blocks.ids().all(|b| !blocks.def(b).has_tag(tag))
            {
                return Err(bad(format!("nothing has the tag `{tag}` it is mended by")));
            }
            if (head.is_some() || !mend.is_empty()) && uses == 0 {
                return Err(bad("a head or mend, but no `uses:N` to wear".into()));
            }
            let burns = match tags.iter().find_map(|t| t.strip_prefix("burns:")) {
                None => 0,
                Some(n) => n.parse::<u16>().ok().filter(|&n| n > 0).ok_or_else(|| bad(format!("`burns:{n}` is not a count of seconds above 0")))?,
            };
            if burns > 0 && uses > 0 {
                return Err(bad("both `uses:N` and `burns:N`: a thing either wears or burns".into()));
            }
            let burnt_to = tags.iter().find_map(|t| t.strip_prefix("burns_to:")).map(item).transpose()?;
            if burnt_to.is_some() && burns == 0 {
                return Err(bad("`burns_to:` but no `burns:N` to burn".into()));
            }
            let held_light = match tags.iter().find_map(|t| t.strip_prefix("held_light:")) {
                None => 0,
                Some(n) => n.parse::<u8>().ok().filter(|n| (1..=crate::light::MAX_LIGHT).contains(n)).ok_or_else(|| bad(format!("`held_light:{n}` is not a light level from 1 to {}", crate::light::MAX_LIGHT)))?,
            };
            // A light set down lights its place at least as well as in the hand.
            if def.light > 0 && held_light > def.light {
                return Err(bad(format!("`held_light:{held_light}` is brighter in hand than its own light {} set down", def.light)));
            }
            burnt.push(burnt_to);
            if uses == 0 && tags.iter().any(|t| t == "tool") && burns == 0 && !tags.iter().any(|t| t == NEVER_WEARS) {
                return Err(bad(format!("a tool with no `uses:N` to wear; give it one, or `{NEVER_WEARS}`")));
            }
            if uses > 0 && mend.is_empty() && tags.iter().any(|t| t == "tool" || t == "weapon") {
                return Err(bad("a tool that wears with nothing to mend it; give it `mend:` one of the materials it is made of".into()));
            }
            heads.push(head);
            mends.push(mend);
            mended_near.push(near);
            let has = |tag: &String| tags.contains(tag);
            let kind = tags.iter().any(|t| t == "tool").then(|| kinds.iter().position(|k| has(&k.name))).flatten();
            let reap = number(tags, "reap:").unwrap_or(0.0);
            if !(0.0..=MAX_REAP as f32).contains(&reap) || reap.fract() != 0.0 {
                return Err(RegistryError(format!("{}: `reap:{reap}` is not a whole number from 0 to {MAX_REAP}", blocks.def(id).name)));
            }
            let reap = reap as u8;
            tools.push(Tool {
                kind,
                tier: number(tags, "tier:").unwrap_or(0.0) as u8,
                speed: number(tags, "speed:").unwrap_or(1.0).max(0.1),
                yield_: number(tags, "yield:").unwrap_or(0.0).max(0.0),
                reap,
                uses,
                burns,
                held_light,
            });
            covered.push(rules.iter().position(|rule| rule.tags.iter().all(has)));
            suited.push(kinds.iter().enumerate().filter(|(_, k)| k.mines.iter().any(has)).map(|(i, _)| i).collect());
        }
        let opens = opened(blocks, &rules, &tools, &covered);
        Ok(Self { kinds, rules, tools, covered, suited, wear, break_wait, savers, heads, mends, mended_near, mend_share: Vec::new(), mend_skill: Vec::new(), mend_firm: Vec::new(), mend_by_tag, burnt, opens, faster_carried })
    }

    /// How tools and weapons wear.
    pub fn wear(&self) -> &Wear {
        &self.wear
    }

    /// Seconds the hand waits after each block it breaks before it starts on the next.
    pub fn break_wait(&self) -> f32 {
        self.break_wait
    }

    /// Seconds breaking `block` (of `hardness`) takes with `item` in hand, the wait after
    /// it included.
    pub fn break_seconds(&self, item: Option<BlockId>, block: BlockId, hardness: f32) -> f32 {
        hardness / self.speed(item, block) + self.break_wait
    }

    /// Of `carried`, the tool that breaks `block` fastest, if it does so at least
    /// `faster_carried` times as fast as `held` (a pickaxe in the pack while a log is hit
    /// at stone, a stone one while the flint one digs coal's neighbour), with how many times.
    pub fn faster_carried(&self, held: Option<BlockId>, block: BlockId, hardness: f32, carried: impl Iterator<Item = BlockId>) -> Option<(BlockId, f32)> {
        if self.faster_carried == 0.0 {
            return None;
        }
        self.fastest_carried(held, block, hardness, carried).filter(|&(_, times)| times >= self.faster_carried)
    }

    /// Of `carried`, the tool good enough for `block` that breaks it fastest, if any is
    /// faster at all than `held` (one only as fast never is), with how many times; of tools
    /// as fast as each other, the first carried.
    pub fn fastest_carried(&self, held: Option<BlockId>, block: BlockId, hardness: f32, carried: impl Iterator<Item = BlockId>) -> Option<(BlockId, f32)> {
        let now = self.break_seconds(held, block, hardness);
        let mut best: Option<(BlockId, f32)> = None;
        for item in carried.filter(|&item| Some(item) != held && self.tool(Some(item)).kind.is_some() && self.good_enough(Some(item), block)) {
            let times = now / self.break_seconds(Some(item), block, hardness);
            if times > 1.0 && best.is_none_or(|(_, b)| times > b) {
                best = Some((item, times));
            }
        }
        best
    }

    /// The slot of `slots` (other than `hand`) holding the spare to bring up for `broken`, a
    /// tool that broke there: its like first, else a tool of its kind, the one with most uses
    /// left; a weapon's spare is only its like, whatever kind it is tagged with.
    pub fn spare(&self, blocks: &BlockRegistry, broken: BlockId, slots: &[Option<crate::inventory::ItemStack>], hand: usize) -> Option<usize> {
        let kind = self.tool(Some(broken)).kind.filter(|_| !blocks.def(broken).has_tag("weapon"));
        let fits = |s: &crate::inventory::ItemStack| s.item == broken || kind.is_some() && self.tool(Some(s.item)).kind == kind;
        slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.map(|s| (i, s)))
            .filter(|&(i, s)| i != hand && fits(&s) && self.uses(s.item) > 0)
            .max_by_key(|&(i, s)| (s.item == broken, self.uses(s.item).saturating_sub(s.wear), std::cmp::Reverse(i)))
            .map(|(i, _)| i)
    }

    /// Seconds `item` burns while held; 0 if it never burns.
    pub fn burns(&self, item: BlockId) -> u16 {
        self.tools.get(item.0 as usize).map_or(0, |tool| tool.burns)
    }

    /// What `item` leaves once burnt out, if anything.
    pub fn burnt(&self, item: BlockId) -> Option<BlockId> {
        self.burnt.get(item.0 as usize).copied().flatten()
    }

    /// The light `item` gives in the hand; 0 for none.
    pub fn held_light(&self, item: BlockId) -> u8 {
        self.tools.get(item.0 as usize).map_or(0, |tool| tool.held_light)
    }

    /// What `item` has to spend, worn by use or burnt by time (an `ItemStack`'s `wear` counts
    /// it down); 0 if it lasts for ever.
    pub fn life(&self, item: BlockId) -> u16 {
        self.uses(item).max(self.burns(item))
    }

    /// Uses `item` lasts; 0 if it never wears.
    pub fn uses(&self, item: BlockId) -> u16 {
        self.tools.get(item.0 as usize).map_or(0, |tool| tool.uses)
    }

    /// Uses breaking `block` takes from `item` (none from a bare hand or what never wears,
    /// and none for a block that breaks at a touch, `hardness` 0).
    pub fn break_wear(&self, item: Option<BlockId>, block: BlockId, hardness: f32) -> u16 {
        let Some(item) = item.filter(|&i| self.uses(i) > 0) else { return 0 };
        if hardness <= 0.0 {
            0
        } else if self.suits(&self.tool(Some(item)), block) {
            self.wear.suited
        } else {
            self.wear.unsuited
        }
    }

    /// The skill whose level saves `item`'s wear: its kind's, else for a weapon
    /// (`weapon`) the wear's weapon skill.
    /// The skill that saves `item`'s wear: its kind's, or for a weapon the skill it is
    /// wielded with (`wielded`, a sling's Ranged), else `Wear::weapon_skill`.
    pub fn wear_skill(&self, item: BlockId, weapon: bool, wielded: Option<SkillId>) -> Option<SkillId> {
        match self.tool(Some(item)).kind {
            Some(kind) => self.savers[kind],
            None if weapon => wielded.or(self.wear.weapon_skill),
            None => None,
        }
    }

    /// Whether a use by a player at `level` in the item's wear skill takes no wear, by a
    /// draw from 0 to 1.
    pub fn saves_wear(&self, level: u8, draw: f32) -> bool {
        draw < level as f32 * self.wear.save_per_level
    }

    /// What is left of `item` when it breaks: its head, if it names one.
    pub fn head(&self, item: BlockId) -> Option<BlockId> {
        self.heads.get(item.0 as usize).copied().flatten()
    }

    /// What mends `item`.
    pub fn mends(&self, item: BlockId) -> &[BlockId] {
        self.mends.get(item.0 as usize).map_or(&[], Vec::as_slice)
    }

    /// The tag of the block `item` is mended by.
    pub fn mend_near(&self, item: BlockId) -> &str {
        self.mended_near.get(item.0 as usize).and_then(Option::as_deref).unwrap_or(&self.wear.mend_near)
    }

    /// Every tag a block mending is done by may have, each once.
    /// The tag whose items mend `item`, if it is mended by a tag (`mend:#TAG`).
    pub fn mend_by_tag(&self, item: BlockId) -> Option<&str> {
        self.mend_by_tag.get(item.0 as usize).and_then(Option::as_deref)
    }

    /// Every tag tools are mended with (`mend:#TAG`), each once.
    pub fn mend_by_tags(&self) -> Vec<&str> {
        let mut tags: Vec<&str> = self.mend_by_tag.iter().flatten().map(String::as_str).collect();
        tags.sort_unstable();
        tags.dedup();
        tags
    }

    pub fn mend_tags(&self) -> Vec<&str> {
        let mut tags: Vec<&str> = std::iter::once(self.wear.mend_near.as_str()).chain(self.mended_near.iter().flatten().map(String::as_str)).filter(|t| !t.is_empty()).collect();
        tags.sort_unstable();
        tags.dedup();
        tags
    }

    /// What the crafting grid makes if it mends a tool: exactly one worn item and one of
    /// what mends it, near (`near` says which tags' blocks are within reach) the block it
    /// is mended by. The tool comes back `Wear::mend` of its uses the better, never past new.
    pub fn mended(&self, grid: &[Option<crate::inventory::ItemStack>; crate::craft::GRID], near: impl Fn(&str) -> bool) -> Option<crate::inventory::ItemStack> {
        let filled: Vec<crate::inventory::ItemStack> = grid.iter().flatten().copied().collect();
        let [a, b] = filled[..] else { return None };
        let (tool, by) = if a.wear > 0 { (a, b) } else { (b, a) };
        let uses = self.uses(tool.item);
        if tool.wear == 0 || uses == 0 || tool.count != 1 || !self.mends(tool.item).contains(&by.item) || !near(self.mend_near(tool.item)) {
            return None;
        }
        let back = ((uses as f32 * self.mend_share(tool.item)).round() as u16).max(1);
        Some(crate::inventory::ItemStack { wear: tool.wear.saturating_sub(back), ..tool })
    }

    /// The share of `item`'s uses one mend brings back: `Wear::mend`, or all a piece of its
    /// recipe's mend material is of it if more (a knife of one ingot comes back whole).
    pub fn mend_share(&self, item: BlockId) -> f32 {
        self.mend_share.get(item.0 as usize).copied().unwrap_or(self.wear.mend)
    }

    /// The skill and level mending `item` asks: what its recipe asks, if anything.
    pub fn mend_skill(&self, item: BlockId) -> Option<(SkillId, u8)> {
        self.mend_skill.get(item.0 as usize).copied().flatten()
    }

    /// Whether mending `item` asks its level with no boost or schematic, its recipe being firm.
    pub fn mend_firm(&self, item: BlockId) -> bool {
        self.mend_firm.get(item.0 as usize).copied().unwrap_or(false)
    }

    /// Ties wear to recipes once all content is loaded: each wearing item's mend share and
    /// skill come from the cheapest recipe that makes it, every weapon must wear (or say
    /// `never_wears`), and making a new one after a break must cost at least what mending
    /// one through a whole life does, or breaking would be a cheap mend.
    pub fn link(content: &crate::content::Content) -> Result<MendTerms, RegistryError> {
        let (blocks, tools) = (&content.blocks, &content.tools);
        let mut known = std::collections::HashMap::new();
        let (mut shares, mut skills, mut firm) = (vec![tools.wear.mend; blocks.len()], vec![None; blocks.len()], vec![false; blocks.len()]);
        for item in blocks.ids() {
            let def = blocks.def(item);
            let uses = tools.uses(item);
            let weapon = def.has_tag("weapon") || content.combat.weapon(item).is_some();
            if weapon && uses == 0 && !def.has_tag(NEVER_WEARS) {
                return Err(RegistryError(format!("`{}`: a weapon with no `uses:N` to wear; give it one, or `{NEVER_WEARS}`", def.name)));
            }
            let mends = tools.mends(item);
            if weapon && uses > 0 && mends.is_empty() {
                return Err(RegistryError(format!("`{}`: a weapon that wears with nothing to mend it; give it `mend:` one of the materials it is made of", def.name)));
            }
            if uses == 0 || mends.is_empty() {
                continue;
            }
            let mut cheapest: Option<(f32, &crate::craft::Recipe)> = None;
            for recipe in content.recipes.iter().filter(|r| r.output == item && r.count > 0) {
                let mut cost = 0.0;
                for cell in recipe.alternatives().into_iter().flatten() {
                    cost += cell.iter().map(|&i| crate::value::worth_in(content, i, &mut known) as f32).reduce(f32::min).unwrap_or(0.0);
                }
                let cost = cost / recipe.count as f32;
                if cheapest.is_none_or(|(least, _)| cost < least) {
                    cheapest = Some((cost, recipe));
                }
            }
            let Some((cost, recipe)) = cheapest else { continue };
            let pieces = recipe.alternatives().into_iter().flatten().filter(|cell| cell.iter().any(|i| mends.contains(i))).count();
            if pieces == 0 {
                return Err(RegistryError(format!(
                    "`{}`: mended with what its recipe has no piece of; a mend puts back what it is made of, so mend it with one of its own materials",
                    def.name
                )));
            }
            let share = tools.wear.mend.max(1.0 / pieces as f32);
            shares[item.0 as usize] = share;
            skills[item.0 as usize] = recipe.skill.map(|skill| (skill, recipe.level));
            firm[item.0 as usize] = recipe.firm;
            // What a break gives back counts off what a new one costs.
            let back = tools.head(item).map_or(0.0, |head| crate::value::worth_in(content, head, &mut known) as f32);
            let mend = mends.iter().map(|&m| crate::value::worth_in(content, m, &mut known) as f32).reduce(f32::min).unwrap_or(0.0);
            let life = (1.0 / share).ceil() * mend;
            if cost - back < life {
                return Err(RegistryError(format!(
                    "`{}`: a new one after it breaks costs {} coins, less than the {life} coins of mending one through a life; mending must never cost more than breaking",
                    def.name,
                    cost - back
                )));
            }
        }
        Ok((shares, skills, firm))
    }

    /// Sets what [`ToolRegistry::link`] found.
    pub fn set_mend_terms(&mut self, (shares, skills, firm): MendTerms) {
        (self.mend_share, self.mend_skill, self.mend_firm) = (shares, skills, firm);
    }

    /// What `item` is as a tool (nothing held is a bare hand).
    pub fn tool(&self, item: Option<BlockId>) -> Tool {
        item.and_then(|item| self.tools.get(item.0 as usize).copied()).unwrap_or(Tool { speed: 1.0, ..Default::default() })
    }

    pub fn kind(&self, index: usize) -> &KindDef {
        &self.kinds[index]
    }

    /// The rule for breaking `block`, if any.
    pub fn rule(&self, block: BlockId) -> Option<&Rule> {
        self.covered.get(block.0 as usize).copied().flatten().map(|i| &self.rules[i])
    }

    /// Whether `tool` is made for `block`.
    pub fn suits(&self, tool: &Tool, block: BlockId) -> bool {
        tool.kind.is_some_and(|kind| self.suited.get(block.0 as usize).is_some_and(|kinds| kinds.contains(&kind)))
    }

    /// How many times as fast as a bare hand `item` breaks `block`.
    pub fn speed(&self, item: Option<BlockId>, block: BlockId) -> f32 {
        let tool = self.tool(item);
        if self.suits(&tool, block) { tool.speed } else { 1.0 }
    }

    /// How far round `block` (a ripe crop) `item` reaps the ripe crops like it: none for a
    /// tool not made for it.
    pub fn reap(&self, item: Option<BlockId>, block: BlockId) -> u8 {
        let tool = self.tool(item);
        if self.suits(&tool, block) { tool.reap } else { 0 }
    }

    /// What `item` adds to the chance of an extra or rarer find from `block`.
    pub fn yield_bonus(&self, item: Option<BlockId>, block: BlockId) -> f32 {
        let tool = self.tool(item);
        if self.suits(&tool, block) { tool.yield_ } else { 0.0 }
    }

    /// Whether `item` is the tool `block`'s rule asks for (any, if it asks for none).
    pub fn good_enough(&self, item: Option<BlockId>, block: BlockId) -> bool {
        let Some(rule) = self.rule(block) else { return true };
        let Some(kind) = rule.tool else { return true };
        let tool = self.tool(item);
        tool.kind == Some(kind) && tool.tier >= rule.tier
    }

    /// Every item that is a `kind` of at least `tier`, with its tier.
    pub fn all_of(&self, kind: usize, tier: u8) -> impl Iterator<Item = (u8, BlockId)> + '_ {
        self.tools.iter().enumerate().filter(move |(_, t)| t.kind == Some(kind) && t.tier >= tier).map(|(i, t)| (t.tier, BlockId(i as u16)))
    }

    /// The chance breaking `block` with `item` gives its rule's `instead`, and what: none
    /// when the rule waits on another kind of tool.
    pub fn instead(&self, item: Option<BlockId>, block: BlockId) -> Option<(BlockId, f32)> {
        let rule = self.rule(block)?;
        let (given, chance) = rule.instead?;
        match rule.by {
            None => Some((given, chance)),
            Some(kind) => {
                let tool = self.tool(item);
                (tool.kind == Some(kind)).then(|| (given, chance + tool.yield_))
            }
        }
    }

    /// The first item, in registry order, that is a `kind` of at least `tier` and of the
    /// lowest such tier. What a refusal names is `Content::least_tool`, which also weighs
    /// worth.
    pub fn first_of(&self, kind: usize, tier: u8) -> Option<BlockId> {
        let mut best: Option<(u8, BlockId)> = None;
        for (i, tool) in self.tools.iter().enumerate() {
            if tool.kind == Some(kind) && tool.tier >= tier && best.is_none_or(|(t, _)| tool.tier < t) {
                best = Some((tool.tier, BlockId(i as u16)));
            }
        }
        best.map(|(_, id)| id)
    }

    /// What `item` breaks that a lesser tool of its kind cannot, nearest level first (see
    /// [`opened`]); none for what is no tool.
    pub fn opens(&self, item: BlockId) -> &[BlockId] {
        self.opens.get(item.0 as usize).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockDef;

    fn setup() -> (BlockRegistry, SkillRegistry, ToolRegistry) {
        let block = |name: &str, tags: &[&str]| BlockDef { name: format!("t:{name}"), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() };
        let blocks = BlockRegistry::new(vec![
            block("air", &[]),
            block("stone", &["stone", "natural"]),
            block("iron_ore", &["ore", "metal:iron", "natural"]),
            block("log", &["log", "wood"]),
            block("flint_pickaxe", &["pickaxe", "tool", "tier:1", "speed:2", "uses:50", "mend:log"]),
            block("stone_pickaxe", &["pickaxe", "tool", "tier:2", "speed:4", "yield:0.05", "uses:100", "mend:stone"]),
            block("stone_axe", &["axe", "tool", "tier:2", "speed:4", "uses:100", "mend:stone"]),
            block("war_axe", &["weapon", "axe", "tool", "tier:2", "speed:4", "uses:100", "mend:stone"]),
        ])
        .unwrap();
        let mut skills = SkillRegistry::default();
        skills.extend("core", r#"(curve: RuneScape(max: 99), skills: [(name: "mining", title: "", description: "")])"#).unwrap();
        let source = r#"(
            kinds: [(name: "pickaxe", mines: ["stone", "ore"]), (name: "axe", mines: ["log"])],
            blocks: [
                (tags: ["ore", "metal:iron"], skill: "core:mining", level: 15, tool: "pickaxe", tier: 2, xp: 35.0),
                (tags: ["stone", "natural"], skill: "core:mining", xp: 3.0),
            ],
        )"#;
        let tools = ToolRegistry::new(vec![ToolRegistry::read("t", source).unwrap()], &blocks, &skills).unwrap();
        (blocks, skills, tools)
    }

    #[test]
    fn tools_speed_up_what_they_suit_only() {
        let (blocks, _, tools) = setup();
        let id = |n: &str| blocks.expect(&format!("t:{n}"));
        assert_eq!(tools.speed(None, id("stone")), 1.0);
        assert_eq!(tools.speed(Some(id("stone_pickaxe")), id("stone")), 4.0);
        assert_eq!(tools.speed(Some(id("stone_pickaxe")), id("log")), 1.0);
        assert_eq!(tools.speed(Some(id("stone_axe")), id("log")), 4.0);
        assert_eq!(tools.speed(Some(id("log")), id("stone")), 1.0, "a log is no tool");
        assert_eq!(tools.yield_bonus(Some(id("stone_pickaxe")), id("iron_ore")), 0.05);
    }

    /// A spare for a tool broken in the hand: its like first, else its kind's with most uses
    /// left; for a weapon, only its like, though it is tagged a tool of a kind.
    #[test]
    fn a_spare_is_its_like_or_its_kind_and_a_weapons_only_its_like() {
        use crate::inventory::ItemStack;
        let (blocks, _, tools) = setup();
        let id = |n: &str| blocks.expect(&format!("t:{n}"));
        let worn = |n: &str, wear: u16| Some(ItemStack { wear, ..ItemStack::new(id(n), 1) });
        let slots = [None, worn("stone_axe", 90), worn("war_axe", 0), worn("stone_axe", 10), worn("flint_pickaxe", 0)];
        assert_eq!(tools.spare(&blocks, id("stone_axe"), &slots, 0), Some(3), "its like with most uses left");
        assert_eq!(tools.spare(&blocks, id("flint_pickaxe"), &slots, 4), None, "not the one in the hand itself");
        assert_eq!(tools.spare(&blocks, id("war_axe"), &[None, worn("stone_axe", 0)], 0), None, "a weapon's spare is only its like");
        assert_eq!(tools.spare(&blocks, id("war_axe"), &slots, 0), Some(2));
        assert_eq!(tools.spare(&blocks, id("stone_pickaxe"), &slots, 0), Some(4), "a tool of its kind");
    }

    #[test]
    fn rules_ask_for_tools_and_levels() {
        let (blocks, skills, tools) = setup();
        let id = |n: &str| blocks.expect(&format!("t:{n}"));
        let iron = tools.rule(id("iron_ore")).unwrap();
        assert_eq!((iron.skill, iron.level, iron.tier, iron.xp), (skills.id("core:mining"), 15, 2, 35.0));
        assert!(!tools.good_enough(None, id("iron_ore")));
        assert!(!tools.good_enough(Some(id("flint_pickaxe")), id("iron_ore")));
        assert!(tools.good_enough(Some(id("stone_pickaxe")), id("iron_ore")));
        assert!(tools.good_enough(None, id("stone")), "stone needs no tool");
        assert!(tools.rule(id("log")).is_none());
        assert_eq!(tools.first_of(iron.tool.unwrap(), 2), Some(id("stone_pickaxe")));
    }

    /// Wear counts up to the uses a tool lasts: it warns once, crossing the share left that
    /// warns, and breaks at none left; what never wears never does.
    #[test]
    fn wear_warns_once_then_breaks() {
        let mut stack = crate::inventory::ItemStack::new(BlockId(1), 1);
        assert_eq!(wear_down(&mut stack, 5, 20, 2), Worn::Fine);
        assert_eq!(wear_down(&mut stack, 12, 20, 2), Worn::Fine, "3 left of 20, still over the 2 it warns at");
        assert_eq!(stack.wear, 17);
        assert_eq!(wear_down(&mut stack, 1, 20, 2), Worn::Warned, "2 left: where it warns");
        assert_eq!(wear_down(&mut stack, 1, 20, 2), Worn::Fine, "warned already");
        assert_eq!(wear_down(&mut stack, 5, 20, 2), Worn::Broke);
        assert_eq!(stack.wear, 20, "never past its uses");
        let mut stone = crate::inventory::ItemStack::new(BlockId(1), 64);
        assert_eq!(wear_down(&mut stone, 3, 0, 0), Worn::Fine);
        // At a tenth left, or `warn_uses` if more, never the whole.
        let wear = Wear { warn_uses: 10, ..Wear::default() };
        assert_eq!([50, 600, 5].map(|uses| wear.warn_at(uses)), [10, 60, 4]);
        assert_eq!(stone.wear, 0);
    }

    /// Load refuses a second `uses:`, a tool that never wears without saying so, and one
    /// that wears with nothing to mend it, and names a head or mend in the item's own
    /// package.
    #[test]
    fn wear_tags_are_checked_at_load() {
        let block = |name: &str, tags: &[&str]| BlockDef { name: name.into(), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() };
        let skills = SkillRegistry::default();
        let load = |tags: &[&str]| {
            let blocks = BlockRegistry::new(vec![block("t:air", &[]), block("t:flint", &[]), block("t:knife", tags)]).unwrap();
            ToolRegistry::new(vec![ToolRegistry::read("t", "(kinds: [], blocks: [])").unwrap()], &blocks, &skills).map(|tools| tools.mends(blocks.expect("t:knife")).to_vec())
        };
        assert_eq!(load(&["tool", "uses:9", "mend:flint"]).unwrap().len(), 1, "`flint` is the knife's own package's");
        assert!(load(&["tool", "uses:9", "uses:8"]).is_err(), "two uses");
        assert!(load(&["tool"]).is_err(), "a tool that never wears without saying so");
        assert!(load(&["tool", NEVER_WEARS]).is_ok());
        let no_mend = load(&["tool", "uses:9"]).unwrap_err().0;
        assert!(no_mend.contains("nothing to mend it"), "a tool that wears needs a mend: {no_mend}");
        let no_mend = load(&["weapon", "uses:9"]).unwrap_err().0;
        assert!(no_mend.contains("nothing to mend it"), "and so does a weapon: {no_mend}");
    }

    /// A mend puts back what a tool is made of: one whose recipe has no piece of what mends
    /// it fails at load, and the game's own all pass.
    #[test]
    fn a_mend_must_be_of_what_the_recipe_uses() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let mut content = crate::content::Content::load(&assets).unwrap();
        assert!(ToolRegistry::link(&content).is_ok());
        let (pick, cobblestone) = (content.blocks.expect("base:iron_pickaxe"), content.blocks.expect("base:cobblestone"));
        let tools = std::sync::Arc::get_mut(&mut content.tools).expect("only the loaded content holds its tools");
        tools.mends[pick.0 as usize] = vec![cobblestone];
        let error = ToolRegistry::link(&content).unwrap_err().to_string();
        assert!(error.contains("iron_pickaxe") && error.contains("no piece of"), "{error}");
    }

    /// What burns held (`burns:N`) lasts that many seconds, counted as wear, leaves what
    /// `burns_to:` names and may light the hand (`held_light:N`); load refuses a thing that
    /// both wears and burns, burns to nothing that exists, lights past full light or lights
    /// brighter in hand than set down. A torch lights the hand and burns nothing down.
    #[test]
    fn what_burns_is_told_by_its_tags_and_checked() {
        let block = |name: &str, tags: &[&str]| BlockDef { name: format!("t:{name}"), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() };
        let skills = SkillRegistry::default();
        let lit = |light: u8, brand: &[&str]| {
            let blocks = BlockRegistry::new(vec![block("air", &[]), block("ash", &[]), BlockDef { light, ..block("brand", brand) }]).unwrap();
            let (brand, ash) = (blocks.expect("t:brand"), blocks.expect("t:ash"));
            ToolRegistry::new(vec![ToolRegistry::read("t", "(kinds: [], blocks: [])").unwrap()], &blocks, &skills).map(|tools| (tools.burns(brand), tools.burnt(brand).map(|b| b == ash), tools.held_light(brand), tools.life(brand)))
        };
        let load = |brand: &[&str]| lit(0, brand);
        assert_eq!(load(&["burns:90", "burns_to:t:ash", "held_light:8"]).unwrap(), (90, Some(true), 8, 90));
        assert_eq!(load(&[]).unwrap(), (0, None, 0, 0), "nothing burns unless it says");
        for bad in [&["burns:0"][..], &["burns:90", "uses:20"], &["burns_to:t:ash"], &["burns:9", "burns_to:t:soot"], &["held_light:16"], &["held_light:0"]] {
            assert!(load(bad).is_err(), "{bad:?} loaded");
        }
        assert_eq!(lit(10, &["held_light:10"]).unwrap().2, 10, "as bright in hand as set down");
        assert!(lit(10, &["held_light:11"]).is_err(), "brighter in hand than set down");
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = crate::content::Content::load(&assets).unwrap();
        let brand = content.blocks.expect("base:firebrand");
        assert_eq!((content.tools.burns(brand), content.tools.burnt(brand), content.tools.held_light(brand)), (90, Some(content.blocks.expect("base:charred_stick")), 8));
        assert_eq!(crate::inventory::max_stack(brand), 1, "what burns is carried one at a time");
        let torch = content.blocks.expect("base:torch");
        assert_eq!((content.tools.held_light(torch), content.tools.burns(torch)), (12, 0), "a torch lights the hand and burns nothing down");
        assert!(content.tools.held_light(torch) > content.tools.held_light(brand), "and lights it better than a firebrand");
    }

    /// Metal tools and weapons mend with their own ingot at a grindstone, not at a
    /// chopping block, where flint and stone ones mend.
    #[test]
    fn metal_mends_at_a_grindstone() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = crate::content::Content::load(&assets).unwrap();
        let (tools, id) = (&content.tools, |name: &str| content.blocks.expect(name));
        for (tool, ingot) in [("base:copper_pickaxe", "base:copper_ingot"), ("base:bronze_axe", "base:bronze_ingot"), ("base:iron_sword", "base:iron_ingot")] {
            let worn = crate::inventory::ItemStack { wear: tools.uses(id(tool)) / 2, ..crate::inventory::ItemStack::new(id(tool), 1) };
            let mut grid = [None; crate::craft::GRID];
            grid[0] = Some(worn);
            grid[4] = Some(crate::inventory::ItemStack::new(id(ingot), 1));
            assert!(tools.mended(&grid, |tag| tag == "grindstone").is_some(), "{tool} at a grindstone");
            assert_eq!(tools.mended(&grid, |tag| tag == "chopping_block"), None, "{tool} at a chopping block");
        }
    }

    /// The game's tools wear, break to nothing and mend by what made them, at a
    /// chopping block and nowhere else, by half their uses, never past new; a grid with
    /// anything else in it, or a tool not worn, mends nothing.
    #[test]
    fn tools_mend_by_half_at_a_chopping_block() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = crate::content::Content::load(&assets).unwrap();
        let (tools, id) = (&content.tools, |name: &str| content.blocks.expect(name));
        let pick = id("base:flint_pickaxe");
        let uses = tools.uses(pick);
        assert!(uses > 0 && tools.head(pick).is_none(), "a break gives nothing of what made it back");
        assert!(tools.mends(pick).contains(&id("base:flint")), "mended with flint");
        assert_eq!(crate::inventory::max_stack(pick), 1, "what wears stacks one at a time");
        let worn = crate::inventory::ItemStack { wear: uses - 3, ..crate::inventory::ItemStack::new(pick, 1) };
        let mut grid = [None; crate::craft::GRID];
        grid[0] = Some(worn);
        grid[4] = Some(crate::inventory::ItemStack::new(id("base:flint"), 5));
        let by_block = |tag: &str| tag == "chopping_block";
        let mended = tools.mended(&grid, by_block).expect("mended by a chopping block");
        assert_eq!((mended.item, mended.wear), (pick, uses - 3 - (uses as f32 * tools.wear().mend).round() as u16));
        assert_eq!(tools.mended(&grid, |_| false), None, "away from one");
        grid[0] = Some(crate::inventory::ItemStack { wear: 2, ..worn });
        assert_eq!(tools.mended(&grid, by_block).map(|m| m.wear), Some(0), "never past new");
        grid[0] = Some(crate::inventory::ItemStack::new(pick, 1));
        assert_eq!(tools.mended(&grid, by_block), None, "a new tool needs no mending");
        grid[0] = Some(worn);
        grid[8] = Some(crate::inventory::ItemStack::new(id("base:stick"), 1));
        assert_eq!(tools.mended(&grid, by_block), None, "something else in the grid");
        grid[8] = None;
        grid[4] = Some(crate::inventory::ItemStack::new(id("base:cobblestone"), 1));
        assert_eq!(tools.mended(&grid, by_block), None, "not what mends it");
        // One flint brings a flint knife (one flint) back whole, a pickaxe (two) half;
        // mending an iron pickaxe asks the level making one does.
        let knife = id("base:flint_knife");
        assert_eq!((tools.mend_share(pick), tools.mend_share(knife)), (0.5, 1.0));
        // A trophy is kept by care: the tusk club by its twine bindings, half at a time.
        let club = id("base:tusk_club");
        assert_eq!((tools.mends(club), tools.mend_share(club)), (&[id("base:twine")][..], 0.5));
        let engineering = content.skills.id("core:engineering").unwrap();
        assert_eq!(tools.mend_skill(id("base:iron_pickaxe")), Some((engineering, 15)));
        // Met as making one is: boosts count, as a schematic would.
        let lacks = |own: u8, boosted: u8| crate::level::lacks_for_mend(tools, id("base:iron_pickaxe"), |_| (own, boosted), &crate::inventory::Inventory::default(), &content.blocks);
        assert_eq!((lacks(14, 14), lacks(14, 15), lacks(15, 15)), (Some((engineering, 15)), None, None));
        grid[0] = Some(crate::inventory::ItemStack { wear: tools.uses(knife) - 1, ..crate::inventory::ItemStack::new(knife, 1) });
        grid[4] = Some(crate::inventory::ItemStack::new(id("base:flint"), 1));
        assert_eq!(tools.mended(&grid, by_block).map(|m| m.wear), Some(0));
        // Every tool and weapon wears (or says it never does) and has a mend that exists,
        // and tools last longer the better their tier.
        for item in content.blocks.ids() {
            let def = content.blocks.def(item);
            let wields = def.has_tag("tool") || def.has_tag("weapon") || content.combat.weapon(item).is_some();
            if wields && !def.has_tag(NEVER_WEARS) {
                assert!(tools.uses(item) > 0 && !tools.mends(item).is_empty(), "{} wears and mends", def.name);
            }
        }
        // What mends by a tag, and where, is named in every language.
        for code in ["en", "ja"] {
            let lang = crate::lang::Lang::load(&assets, code);
            for key in tools.mend_by_tags().iter().map(|t| format!("item.mend.tag.{t}")).chain(tools.mend_tags().iter().map(|t| format!("item.mend.near.{t}"))) {
                assert!(lang.defines(&key), "{code} names {key}");
            }
        }
        // Tier 1 is flint, or the digging stick; there is no tier-1 hoe.
        for (kind, first) in [("pickaxe", Some("flint_pickaxe")), ("axe", Some("flint_axe")), ("shovel", Some("digging_stick")), ("hoe", None), ("knife", Some("flint_knife"))] {
            let tiers = ["stone", "copper", "iron"].map(|tier| format!("{tier}_{kind}"));
            let lasts: Vec<u16> = first.map(String::from).iter().chain(&tiers).map(|name| tools.uses(id(&format!("base:{name}")))).collect();
            assert!(lasts.windows(2).all(|w| w[0] < w[1]), "{kind}s last longer the better they are: {lasts:?}");
        }
    }

    /// A dead snag's trunk (`base:dead_log`) is never an item: it drops nothing of its own
    /// and breaking it gives a stick instead, every time, and nothing else always breaks to
    /// sticks: natural leaves give one only sometimes (the hafts of a bare-handed player's
    /// first tools), and nothing else does at all. It is weathered wood on every face, as its
    /// limbs and fallen trunks are made of it too.
    #[test]
    fn dead_logs_break_to_sticks() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = crate::content::Content::load(&assets).unwrap();
        let (dead, stick) = (content.blocks.expect("base:dead_log"), content.blocks.expect("base:stick"));
        let info = content.blocks.info(dead);
        assert!(info.drops.is_air());
        assert!(info.layers.iter().all(|&layer| layer == info.layers[0]), "a dead log looks the same on every face: {:?}", info.layers);
        let rule = content.tools.rule(dead).expect("a rule for dead logs");
        assert_eq!(rule.instead, Some((stick, 1.0)));
        let others: Vec<(&str, f32)> = content
            .blocks
            .ids()
            .filter(|&b| b != dead)
            .filter_map(|b| content.tools.rule(b).and_then(|r| r.instead).filter(|&(given, _)| given == stick).map(|(_, chance)| (content.blocks.def(b).name.as_str(), chance)))
            .collect();
        let leaves = |name: &str| content.blocks.def(content.blocks.expect(name)).has_tag("leaves");
        assert!(others.iter().any(|&(name, _)| leaves(name)), "natural leaves give sticks sometimes, for the first tools");
        let wrong: Vec<_> = others.iter().filter(|&&(name, chance)| !leaves(name) || chance >= 1.0).collect();
        assert!(wrong.is_empty(), "only dead logs always break to sticks, and only leaves sometimes, not {wrong:?}");
    }

    /// Bare hands break neither natural stone nor a standing log (Mariano, Oct 15: the first
    /// tool must matter), and what they need is made from what bare hands gather: the least
    /// tool each rule names has a recipe the hand's grid makes, every ingredient of which a
    /// block no tool is asked for gives, as its drop or `instead`.
    #[test]
    fn the_first_tools_come_from_bare_hands() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = crate::content::Content::load(&assets).unwrap();
        let (blocks, tools) = (&content.blocks, &content.tools);
        let by_hand = |item: BlockId| {
            blocks.ids().filter(|&b| !blocks.def(b).item && blocks.info(b).hardness > 0.0).any(|b| {
                let rule = tools.rule(b);
                rule.is_none_or(|r| r.tool.is_none() && r.level <= 1) && (blocks.info(b).drops == item || rule.and_then(|r| r.instead).is_some_and(|(given, _)| given == item))
            })
        };
        for name in ["base:stone", "base:oak_log"] {
            let block = blocks.expect(name);
            assert!(!tools.good_enough(None, block), "{name} breaks by bare hand");
            let rule = tools.rule(block).expect("a rule");
            let tool = content.least_tool(rule.tool.expect("asks for a tool"), rule.tier).expect("a tool that does");
            // Named by property: no tool that would do is of a lower tier, or as low and cheaper.
            let worth = |t: BlockId| crate::value::worth(&content, t);
            let least = (tools.tool(Some(tool)).tier, worth(tool));
            let cheaper: Vec<&str> = tools.all_of(rule.tool.unwrap(), rule.tier).filter(|&(tier, t)| (tier, worth(t)) < least).map(|(_, t)| blocks.def(t).name.as_str()).collect();
            assert!(cheaper.is_empty(), "{name}'s refusal names the {}, but {cheaper:?} would do for less", blocks.def(tool).name);
            let recipe = content.recipes.iter().find(|r| r.output == tool && !r.needs_table());
            let recipe = recipe.unwrap_or_else(|| panic!("{} has no recipe the hand's grid makes", blocks.def(tool).name));
            let unreachable: Vec<&str> = recipe.cells().into_iter().flatten().filter(|&i| !by_hand(i)).map(|i| blocks.def(i).name.as_str()).collect();
            assert!(unreachable.is_empty(), "{}: bare hands gather no {unreachable:?}", blocks.def(tool).name);
            assert!(tools.good_enough(Some(tool), block), "the {} breaks {name}", blocks.def(tool).name);
        }
        // Cobble, the hand's own building stone, mossy too where the world grew it.
        let refused: Vec<&str> = blocks.tagged("cobbled").filter(|&b| !tools.good_enough(None, b)).map(|b| blocks.def(b).name.as_str()).collect();
        assert!(refused.is_empty(), "cobble a bare hand cannot break: {refused:?}");
    }

    /// Coal wants a stone pickaxe, deep coal too, while its sample on the ground lifts with
    /// a flint one: the seam is found before it can be dug. Every sample of ore that teaches
    /// prospecting is tagged as one, so the experience goes only with a find.
    #[test]
    fn coal_wants_stone_and_its_sample_flint() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = crate::content::Content::load(&assets).unwrap();
        let (blocks, tools) = (&content.blocks, &content.tools);
        let id = |n: &str| blocks.expect(&format!("base:{n}"));
        let (flint, stone) = (Some(id("flint_pickaxe")), Some(id("stone_pickaxe")));
        for seam in ["coal_ore", "deepslate_coal_ore"] {
            assert!(!tools.good_enough(flint, id(seam)), "a flint pickaxe digs {seam}");
            assert!(tools.good_enough(stone, id(seam)), "a stone pickaxe does not dig {seam}");
        }
        assert!(tools.good_enough(flint, id("coal_ore_sample")), "a flint pickaxe cannot lift a coal sample");
        assert!(tools.rule(id("coal_ore_sample")).is_some_and(|r| r.prospect > 0.0), "a coal sample teaches no prospecting");
        let samples: Vec<&str> = blocks.tagged("ore_sample").filter(|&b| tools.rule(b).is_none_or(|r| r.prospect <= 0.0)).map(|b| blocks.def(b).name.as_str()).collect();
        assert!(samples.is_empty(), "samples that teach no prospecting: {samples:?}");
    }

    /// Tall grass gives twine only to a knife, and a better knife finds it more often; by
    /// hand or with another tool it gives nothing instead. A rule's `by` must name a kind
    /// and come with an `instead`.
    #[test]
    fn tall_grass_gives_twine_to_a_knife() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = crate::content::Content::load(&assets).unwrap();
        let id = |n: &str| content.blocks.expect(&format!("base:{n}"));
        let twine = id("twine");
        for grass in [id("tall_grass"), id("tall_grass_top")] {
            assert_eq!(content.tools.instead(None, grass), None, "by hand");
            assert_eq!(content.tools.instead(Some(id("stone_axe")), grass), None, "with an axe");
            let chances: Vec<f32> = ["flint_knife", "stone_knife", "copper_knife", "iron_knife"]
                .iter()
                .map(|knife| content.tools.instead(Some(id(knife)), grass).filter(|&(given, _)| given == twine).expect("twine with a knife").1)
                .collect();
            assert!(chances.windows(2).all(|w| w[0] < w[1]) && chances[0] >= 0.2, "{chances:?}");
        }
        assert_eq!(content.tools.instead(None, id("short_grass")), None, "short grass gives no twine");
        assert_eq!(content.tools.instead(None, id("wild_wheat")), None, "wild wheat forages as before");
        assert_eq!(content.tools.instead(None, id("gravel")).map(|(given, _)| given), Some(id("flint")), "gravel's flint needs no tool");

        let (blocks, skills, _) = setup();
        for (source, error) in [
            (r#"(kinds: [(name: "knife", mines: [])], blocks: [(tags: ["log"], by: "knife")])"#, "nothing `instead`"),
            (r#"(kinds: [], blocks: [(tags: ["log"], instead: ("t:stone", 0.5), by: "sickle")])"#, "no kind of tool `sickle`"),
        ] {
            let err = ToolRegistry::new(vec![ToolRegistry::read("t", source).unwrap()], &blocks, &skills).unwrap_err();
            assert!(err.0.contains(error), "{}", err.0);
        }
    }

    /// A stone or better hoe reaps the ripe crops about the one it cuts, an iron one a
    /// wider patch; a bare hand and other tools reap only the one.
    #[test]
    fn better_hoes_reap_round_a_crop() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = crate::content::Content::load(&assets).unwrap();
        let id = |n: &str| content.blocks.expect(&format!("base:{n}"));
        let wheat = id("wheat_crop");
        let reaps: Vec<u8> = ["flint_knife", "stone_hoe", "copper_hoe", "iron_hoe"].iter().map(|hoe| content.tools.reap(Some(id(hoe)), wheat)).collect();
        assert_eq!(reaps, [0, 1, 1, 2]);
        assert_eq!(content.tools.reap(None, wheat), 0, "by hand");
        assert_eq!(content.tools.reap(Some(id("iron_hoe")), id("stone")), 0, "a hoe reaps only what it is made for");
        assert_eq!(content.tools.reap(Some(id("iron_axe")), wheat), 0, "an axe reaps nothing");
        assert!(content.blocks.ids().all(|b| content.tools.tool(Some(b)).reap <= MAX_REAP));

        // Wider than MAX_REAP, or a fraction, is refused at load.
        let (_, skills, _) = setup();
        for tag in ["reap:3", "reap:1.5"] {
            let hoe = BlockDef { name: "t:hoe".into(), tags: ["hoe", "tool", "never_wears", tag].iter().map(|t| t.to_string()).collect(), ..Default::default() };
            let blocks = BlockRegistry::new(vec![BlockDef { name: "t:air".into(), ..Default::default() }, hoe]).unwrap();
            let file = ToolRegistry::read("t", r#"(kinds: [(name: "hoe", mines: ["crop"])])"#).unwrap();
            let err = ToolRegistry::new(vec![file], &blocks, &skills).unwrap_err();
            assert!(err.0.contains("not a whole number"), "{tag}: {}", err.0);
        }
    }

    /// Stone tools want hard rock (Mariano, Oct 15: stone should take searching): every
    /// recipe of a tier-2 tool asks for something made only near a workshop block from
    /// toolstone, never plain cobble; that block is made in the hand's grid; and toolstone is
    /// found as natural rock, deepslate among it so mining down reaches it anywhere, while the
    /// commonest stone is not.
    #[test]
    fn stone_tools_take_toolstone_ground_at_a_workshop() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = crate::content::Content::load(&assets).unwrap();
        let blocks = &content.blocks;
        let toolstone = |item: BlockId| blocks.def(item).has_tag("toolstone");
        let natural: Vec<&str> = blocks.tagged("toolstone").filter(|&b| blocks.def(b).has_tag("natural")).map(|b| blocks.def(b).name.as_str()).collect();
        assert!(natural.contains(&"base:deepslate"), "deepslate, under every province, is no toolstone: {natural:?}");
        assert!(!toolstone(blocks.expect("base:stone")) && !toolstone(blocks.expect("base:cobblestone")), "plain stone takes an edge");
        let tools: Vec<BlockId> = blocks.tagged("tool").filter(|&t| blocks.def(t).has_tag("tier:2")).collect();
        assert!(!tools.is_empty(), "no tier-2 tools");
        for tool in tools {
            let name = &blocks.def(tool).name;
            let recipes: Vec<&crate::craft::Recipe> = content.recipes.iter().filter(|r| r.output == tool).collect();
            assert!(!recipes.is_empty(), "{name} has no recipe");
            for recipe in recipes {
                let ground: Vec<BlockId> = recipe.alternatives().into_iter().flatten().flatten().copied().filter(|&i| {
                    let makes: Vec<&crate::craft::Recipe> = content.recipes.iter().filter(|r| r.output == i).collect();
                    !makes.is_empty() && makes.iter().all(|r| r.near.is_some() && r.alternatives().into_iter().flatten().all(|cell| cell.iter().all(|&c| toolstone(c))))
                }).collect();
                let head = ground.first().unwrap_or_else(|| panic!("{name} is made without a head ground from toolstone"));
                let near = content.recipes.iter().find(|r| r.output == *head).and_then(|r| r.near.clone()).unwrap();
                let workshop = blocks.tagged(&near).find(|&b| content.recipes.iter().any(|r| r.output == b && !r.needs_table()));
                assert!(workshop.is_some(), "no block tagged {near} is made in the hand's grid, for {name}");
            }
        }
    }

    /// Each tier of each kind is felt (Oct 14 tier audit, docs/tools-and-crafting.md): on
    /// the blocks tools are for (a log, cobblestone, an ore, soil), a block with the next
    /// tier takes at most 1/1.35 of the time with the one before, counting the wait after
    /// each break (`tools.ron`'s `break_wait`). Bare hands to flint (a digging stick for a
    /// shovel), flint to stone, stone to copper, copper to iron, for the axe, the pickaxe and
    /// the shovel. Soil breaks so fast that past a stone shovel the wait is most of a break:
    /// those steps alone are pinned as not felt, still faster, and near the floor (no tool
    /// could make the copper one felt, an instant one barely 1.4x), so a step newly unfelt fails.
    #[test]
    fn every_tool_tier_is_felt() {
        const FELT: f32 = 1.35;
        const UNFELT: [(&str, usize); 6] = [("dirt", 2), ("dirt", 3), ("sand", 2), ("sand", 3), ("gravel", 2), ("gravel", 3)];
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = crate::content::Content::load(&assets).unwrap();
        let id = |n: &str| content.blocks.expect(&format!("base:{n}"));
        let wait = content.tools.break_wait();
        let tiers = |kind: &str| ["flint", "stone", "copper", "iron"].map(|tier| format!("{tier}_{kind}"));
        let mut shovels = tiers("shovel");
        shovels[0] = "digging_stick".into();
        let mut unfelt = Vec::new();
        for (kind, held, blocks) in [("axe", tiers("axe"), ["oak_log"].as_slice()), ("pickaxe", tiers("pickaxe"), ["cobblestone", "coal_ore"].as_slice()), ("shovel", shovels, ["dirt", "sand", "gravel"].as_slice())] {
            for &block in blocks {
                let hardness = content.blocks.info(id(block)).hardness;
                let held: Vec<Option<BlockId>> = std::iter::once(None).chain(held.iter().map(|item| Some(id(item)))).collect();
                let times: Vec<f32> = held.iter().map(|&item| hardness / content.tools.speed(item, id(block)) + wait).collect();
                for (step, pair) in times.windows(2).enumerate() {
                    if pair[0] / pair[1] >= FELT {
                        continue;
                    }
                    assert!(UNFELT.contains(&(block, step)), "{block}: tier {step} to {} with a {kind} is only {:.2}x ({times:?}, waiting {wait} s)", step + 1, pair[0] / pair[1]);
                    assert!(pair[1] < pair[0] && pair[0] / wait < 1.5, "{block}: tier {step} to {} with a {kind} is slower, or not near the {wait} s floor ({times:?})", step + 1);
                    unfelt.push((block, step));
                }
            }
        }
        assert_eq!(unfelt, UNFELT, "the steps soil leaves no room to feel");
    }

    /// A pickaxe carried while cobblestone is broken with an axe or by hand is found; one no
    /// faster by the share is not (a stone pickaxe over a flint one), nor what is no tool,
    /// nor the one in hand; a package's 0 finds nothing, and a share under 1 is refused.
    #[test]
    fn a_faster_tool_carried_is_found() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = crate::content::Content::load(&assets).unwrap();
        let id = |n: &str| content.blocks.expect(&format!("base:{n}"));
        let tools = &content.tools;
        let (stone, hardness) = (id("cobblestone"), content.blocks.info(id("cobblestone")).hardness);
        let pack = [id("flint_pickaxe"), id("oak_log"), id("stone_pickaxe")];
        let (best, times) = tools.faster_carried(Some(id("flint_axe")), stone, hardness, pack.into_iter()).unwrap();
        assert_eq!(best, id("stone_pickaxe"));
        assert!((times - tools.break_seconds(Some(id("flint_axe")), stone, hardness) / tools.break_seconds(Some(best), stone, hardness)).abs() < 1e-4);
        assert_eq!(tools.faster_carried(None, stone, hardness, pack.into_iter()).map(|(b, _)| b), Some(id("stone_pickaxe")), "by hand");
        // Under the share, nothing: a stone pickaxe over the flint one in hand.
        assert!(tools.break_seconds(Some(id("flint_pickaxe")), stone, hardness) / tools.break_seconds(Some(id("stone_pickaxe")), stone, hardness) < DEFAULT_FASTER_CARRIED);
        assert_eq!(tools.faster_carried(Some(id("flint_pickaxe")), stone, hardness, pack.into_iter()), None);
        // Yet it is the fastest carried, which the middle button picks by any share.
        assert_eq!(tools.fastest_carried(Some(id("flint_pickaxe")), stone, hardness, pack.into_iter()).map(|(b, _)| b), Some(id("stone_pickaxe")));
        assert_eq!(tools.fastest_carried(Some(id("stone_pickaxe")), stone, hardness, pack.into_iter()), None, "none faster than in hand");
        assert_eq!(tools.faster_carried(Some(id("stone_pickaxe")), stone, hardness, pack.into_iter()), None);
        assert_eq!(tools.faster_carried(None, stone, hardness, [id("oak_log")].into_iter()), None);

        // A package's 0 tells of none; what is neither 0 nor from 1 to 100 is refused.
        let (blocks, skills, _) = setup();
        let with = |given: &str| ToolRegistry::new(vec![ToolRegistry::read("t", &format!(r#"(faster_carried: {given}, kinds: [(name: "pickaxe", mines: ["stone"])])"#)).unwrap()], &blocks, &skills);
        let found = |given: &str| with(given).unwrap().faster_carried(None, blocks.expect("t:stone"), 1.0, blocks.ids()).map(|(b, _)| b);
        assert_eq!(found("2.0"), Some(blocks.expect("t:stone_pickaxe")));
        assert_eq!(found("0.0"), None);
        for bad in ["0.5", "101.0", "-1.0"] {
            assert!(with(bad).unwrap_err().0.contains("faster_carried"), "{bad}");
        }

        // At 1, any faster tool is told of, never one only as fast; of two as fast as each
        // other, the first carried is named.
        let both = ToolRegistry::new(vec![ToolRegistry::read("t", r#"(faster_carried: 1.0, kinds: [(name: "pickaxe", mines: ["stone"]), (name: "axe", mines: ["stone"])])"#).unwrap()], &blocks, &skills).unwrap();
        let [stone, pick, axe] = ["t:stone", "t:stone_pickaxe", "t:stone_axe"].map(|n| blocks.expect(n));
        assert_eq!(both.faster_carried(Some(pick), stone, 1.0, [axe].into_iter()), None, "only as fast");
        assert_eq!(both.faster_carried(None, stone, 1.0, [pick, axe].into_iter()).map(|(b, _)| b), Some(pick), "the first of equals");
        assert_eq!(both.faster_carried(None, stone, 1.0, [axe, pick].into_iter()).map(|(b, _)| b), Some(axe), "the first of equals");
        assert_eq!(both.faster_carried(Some(blocks.expect("t:flint_pickaxe")), stone, 1.0, [axe].into_iter()).map(|(b, _)| b), Some(axe), "any faster");
    }

    /// A better pickaxe names what it opens up, plain ore standing for its deepslate form and
    /// its sample, nearest level first; the first of a kind opens nothing a lesser one kept
    /// shut, and a tier no rule asks for opens nothing.
    #[test]
    fn a_better_tool_names_what_it_opens() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = crate::content::Content::load(&assets).unwrap();
        let id = |n: &str| content.blocks.expect(&format!("base:{n}"));
        let opens = |tool: &str| content.tools.opens(id(tool)).iter().map(|&b| content.blocks.def(b).name.trim_start_matches("base:").to_owned()).collect::<Vec<_>>();
        assert_eq!(opens("flint_pickaxe"), Vec::<String>::new());
        assert_eq!(opens("flint_axe"), Vec::<String>::new());
        assert_eq!(opens("stone_pickaxe")[..3], ["coal_ore", "copper_ore", "tin_ore"]);
        assert!(opens("stone_pickaxe").contains(&"iron_ore".to_owned()));
        assert_eq!(opens("copper_pickaxe"), ["cobalt_ore", "silver_ore", "gold_ore"]);
        assert_eq!(opens("stone_axe"), Vec::<String>::new());
        // Bronze is copper's tier and faster: it opens nothing copper did not.
        assert_eq!(opens("bronze_pickaxe"), Vec::<String>::new());
        assert_eq!(opens("iron_pickaxe"), ["ruby_ore", "sapphire_ore", "lodestone", "emerald_ore", "graphite_ore", "diamond_ore", "aetherite_ore", "bauxite"]);
        assert_eq!(opens("oak_log"), Vec::<String>::new());
    }

    /// What Mining teaches rises with the level it asks (design draft 63): nothing a lower
    /// level breaks pays more than what a higher one does, so stone and coal never outpay
    /// the ores above them.
    #[test]
    fn mining_pays_more_for_what_asks_more() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = crate::content::Content::load(&assets).unwrap();
        let mining = content.skills.id("core:mining");
        let mut taught: Vec<(u8, f32, String)> = content
            .blocks
            .ids()
            .filter_map(|b| content.tools.rule(b).filter(|r| r.skill == mining && r.xp > 0.0).map(|r| (r.level.max(1), r.xp, content.blocks.def(b).name.clone())))
            .collect();
        taught.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let out_of_order: Vec<String> = taught
            .windows(2)
            .filter(|pair| pair[0].0 < pair[1].0 && pair[0].1 > pair[1].1)
            .map(|pair| format!("{} (Mining {}, {} xp) pays more than {} (Mining {}, {} xp)", pair[0].2, pair[0].0, pair[0].1, pair[1].2, pair[1].0, pair[1].1))
            .collect();
        assert!(out_of_order.is_empty(), "Mining xp out of level order: {out_of_order:#?}");
    }
}
