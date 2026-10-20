//! Tools: what a held item does to breaking blocks, as content (each package's `tools.ron`).
//!
//! A tool is an item tagged `tool`, with its kind's name (`pickaxe`), a `tier:N`, a
//! `speed:X` and maybe a `yield:Y`. Each kind `mines` some block tags: over a block with
//! one, the tool breaks it `speed` times as fast as a bare hand, and its `yield` adds to the
//! chance of an extra drop (and of rarer harvests, for a hoe over crops). Block rules say
//! what breaking a block takes and teaches: a level in a skill, a tool of a kind and tier,
//! and the experience it gives, RuneScape's mining crossed with Minecraft's tool tiers.
//! Better tools make better finds and quicker work; nothing makes the world grow faster.

use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::skill::{SkillId, SkillRegistry};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KindDef {
    pub name: String,
    /// Block tags it is made for.
    pub mines: Vec<String>,
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
}

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
}

/// What a held item is as a tool.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tool {
    /// Its kind, by index, if it is a tool at all.
    pub kind: Option<usize>,
    pub tier: u8,
    pub speed: f32,
    pub yield_: f32,
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
}

impl ToolRegistry {
    pub fn read(namespace: &str, source: &str) -> Result<ToolFile, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/tools.ron: {e}")))
    }

    pub fn new(files: Vec<ToolFile>, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        let (mut kinds, mut defs) = (Vec::new(), Vec::new());
        for file in files {
            kinds.extend(file.kinds);
            defs.extend(file.blocks);
        }
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
                Ok(Rule { tags: def.tags, skill, level: def.level, tool, tier: def.tier, xp: def.xp, instead })
            })
            .collect::<Result<Vec<_>, RegistryError>>()?;
        let number = |tags: &[String], prefix: &str| tags.iter().find_map(|t| t.strip_prefix(prefix)?.parse::<f32>().ok());
        let mut tools = Vec::with_capacity(blocks.len());
        let mut covered = Vec::with_capacity(blocks.len());
        let mut suited = Vec::with_capacity(blocks.len());
        for id in blocks.ids() {
            let tags = &blocks.def(id).tags;
            let has = |tag: &String| tags.contains(tag);
            let kind = tags.iter().any(|t| t == "tool").then(|| kinds.iter().position(|k| has(&k.name))).flatten();
            tools.push(Tool {
                kind,
                tier: number(tags, "tier:").unwrap_or(0.0) as u8,
                speed: number(tags, "speed:").unwrap_or(1.0).max(0.1),
                yield_: number(tags, "yield:").unwrap_or(0.0).max(0.0),
            });
            covered.push(rules.iter().position(|rule| rule.tags.iter().all(has)));
            suited.push(kinds.iter().enumerate().filter(|(_, k)| k.mines.iter().any(has)).map(|(i, _)| i).collect());
        }
        Ok(Self { kinds, rules, tools, covered, suited })
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
            block("flint_pickaxe", &["pickaxe", "tool", "tier:1", "speed:2"]),
            block("stone_pickaxe", &["pickaxe", "tool", "tier:2", "speed:4", "yield:0.05"]),
            block("stone_axe", &["axe", "tool", "tier:2", "speed:4"]),
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
    /// the blocks tools are for (a log, cobblestone, an ore), a block with the next tier
    /// takes at most 1/1.35 of the time with the one before, counting the quarter second the
    /// hand waits after each break (the client's `mine_cooldown`). Bare hands to flint, flint to
    /// stone, stone to copper, copper to iron.
    #[test]
    fn every_tool_tier_is_felt() {
        const BREAK_WAIT: f32 = 0.25;
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = crate::content::Content::load(&assets).unwrap();
        let id = |n: &str| content.blocks.expect(&format!("base:{n}"));
        for (kind, blocks) in [("axe", ["oak_log"].as_slice()), ("pickaxe", ["cobblestone", "coal_ore"].as_slice())] {
            for block in blocks {
                let hardness = content.blocks.info(id(block)).hardness;
                let held: Vec<Option<BlockId>> = std::iter::once(None).chain(["flint", "stone", "copper", "iron"].iter().map(|tier| Some(id(&format!("{tier}_{kind}"))))).collect();
                let seconds: Vec<f32> = held.iter().map(|&item| hardness / content.tools.speed(item, id(block)) + BREAK_WAIT).collect();
                for (step, pair) in seconds.windows(2).enumerate() {
                    assert!(pair[0] / pair[1] >= 1.35, "{block}: tier {step} to {} with a {kind} is only {:.2}x ({seconds:?})", step + 1, pair[0] / pair[1]);
                }
            }
        }
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
