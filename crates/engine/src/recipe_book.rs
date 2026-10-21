//! The recipe book: every way to get an item and everything an item goes into, as a player
//! looks them up (hover an item, R for its recipes, U for its uses, as in NEI). It is read
//! from the same content the game plays by (crafting recipes, cooking, machines, home
//! stations, crops, what breaking a block gives, fishing, what kept creatures give and what
//! hunted ones drop, traps, bees, locked chests, parties met on the road, quest rewards and
//! what the world lays), so a package's recipes appear in it with no more work. Every item
//! a player could hold has a way in it or is tagged [`CREATIVE_ONLY`]: content with a dead
//! entry fails to load, naming the item and its package.

use rustc_hash::FxHashMap;

use crate::block::BlockId;
use crate::content::Content;
use crate::craft::GRID;
use crate::creature::CreatureId;

/// How an item is got.
#[derive(Clone, Debug, PartialEq)]
pub enum Method {
    /// Laid into the crafting grid, cell by cell, row by row; shapeless recipes take
    /// their ingredients in any arrangement. A family of recipes making the same thing
    /// from different woods (or stones) is one entry with each way laid out, as NEI
    /// cycles them, and a cell that takes any of several items shows each in turn. `first`
    /// is the first of its `ways` recipes, in content order (the rest follow it), for
    /// laying one into the grid. `near` is the tag of a block it is made near, if any; the
    /// level in `skill` it takes, if any.
    Craft { variants: Vec<[Option<BlockId>; GRID]>, shapeless: bool, first: usize, ways: usize, near: Option<String>, skill: Option<crate::skill::SkillId>, level: u8 },
    /// Cooked (or fired, or smelted) at a station (blocks with the station's tag), from
    /// `level` in `skill`: `uses` of the held `input` and the other items `with` it; what
    /// is left when it burns, if anything.
    Cook { input: BlockId, uses: u8, with: Vec<(BlockId, u8)>, station: String, skill: Option<crate::skill::SkillId>, level: u8, burnt: Option<BlockId> },
    /// Taken from a kept creature with a tool (items with the tool's tag), or left by it
    /// without one (eggs).
    Product { creature: CreatureId, tool: Option<String> },
    /// Dropped by a creature when it dies, `chance` of the time.
    Drop { creature: CreatureId, chance: f32 },
    /// Worked by a machine (any of `machines`, from electric tier `tier`): what goes in
    /// (the first item each input takes) and how many; `chance` of the time for a
    /// byproduct, else always.
    Process { machines: Vec<BlockId>, inputs: Vec<(BlockId, u8)>, tier: u8, chance: f32 },
    /// Left in a home station (its empty block) to work: `takes` of `input`, from `level`
    /// in `skill`.
    Work { station: BlockId, input: BlockId, takes: u8, skill: crate::skill::SkillId, level: u8 },
    /// Harvested from a ripe crop (its ripe block), or foraged from a wild one, `chance`
    /// of the time; it is planted from `level` in `skill`. `from` is what the card shows
    /// going in: the seed that plants it (or the crop, if players don't plant it), or the
    /// wild plant foraged.
    Harvest { crop: BlockId, from: BlockId, forage: bool, chance: f32, skill: crate::skill::SkillId, level: u8 },
    /// Got by breaking `block`, with `tool` (the least that does) if it needs one, from
    /// `level` in `skill`; `chance` of the time when it comes instead of the block's own
    /// drop, else always.
    Break { block: BlockId, tool: Option<BlockId>, skill: Option<crate::skill::SkillId>, level: u8, chance: f32 },
    /// Caught on a line, from `level` in `skill`.
    Catch { skill: Option<crate::skill::SkillId>, level: u8 },
    /// Taken from a sprung `trap` (its set block) that caught `creature`: that creature's
    /// loot, `chance` of the times one steps in (the trap's own chance of holding it, by
    /// the loot's), from `level` in `skill`.
    Trap { trap: BlockId, creature: CreatureId, chance: f32, skill: crate::skill::SkillId, level: u8 },
    /// Taken from bees in `block` (a wild nest, a full skep), with `holding` in hand if the
    /// take asks for it, `chance` of the time, from `level` in `skill`.
    Bees { block: BlockId, holding: Option<BlockId>, chance: f32, skill: crate::skill::SkillId, level: u8 },
    /// Found in a locked chest (its block), `chance` of the time.
    Chest { chest: BlockId, chance: f32 },
    /// Given by a party met on the road (the wandering event `event`): for `wants` when it
    /// asks for something, else for meeting, helping or watching it; `chance` of the time.
    Trade { event: usize, wants: Option<(crate::wander::Want, u8)>, chance: f32 },
    /// Given for finishing the quest `quest`.
    Reward { quest: String },
    /// Left when `from` burns out, set down or in hand (a torch's stub), or, `relit`, what
    /// `from` is lit again into (a lamp filled with fat), using up an item tagged `fuel`
    /// if lighting it does.
    Burn { from: BlockId, relit: bool, fuel: Option<String> },
    /// Left where `from` was charred in the structure `structure` (a clamp's firewood), lit
    /// and left to smoulder; `early` if it was opened before its time.
    Smoulder { structure: String, from: BlockId, early: bool },
    /// Found in the world (its generator lays it) and broken loose as itself, with `tool`
    /// (the least that does) if it needs one, from `level` in `skill`.
    Found { tool: Option<BlockId>, skill: Option<crate::skill::SkillId>, level: u8 },
}

/// The tag that marks an item no way in survival gives: the creative catalogue's alone
/// (a block's other states, leaves no tool takes whole). Every other item a player could
/// hold has a way in the book, or content fails to load ([`RecipeBook::unsourced`]).
pub const CREATIVE_ONLY: &str = "creative_only";

impl Method {
    /// The blocks a way is worked at: the blocks tagged with a recipe's `near` or a cooking
    /// station's tag, the machines, the home station, the trap, the bees, the chest. One
    /// path for every station, read from data, so a station whose recipes a package adds
    /// later (an anvil, a quern) lists them with no change here.
    pub fn stations(&self, blocks: &crate::block::BlockRegistry) -> Vec<BlockId> {
        match self {
            Method::Craft { near: Some(tag), .. } | Method::Cook { station: tag, .. } => blocks.tagged(tag).collect(),
            Method::Process { machines, .. } => machines.clone(),
            Method::Work { station, .. } => vec![*station],
            Method::Trap { trap, .. } => vec![*trap],
            Method::Bees { block, .. } => vec![*block],
            Method::Chest { chest, .. } => vec![*chest],
            _ => Vec::new(),
        }
    }
}

/// One way to get an item: how, what comes out, and how many (at least, at most).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub method: Method,
    pub output: BlockId,
    pub count: (u8, u8),
}

/// Every entry, found by what it makes and by what it uses.
#[derive(Debug, Default)]
pub struct RecipeBook {
    entries: Vec<Entry>,
    makes: FxHashMap<BlockId, Vec<usize>>,
    uses: FxHashMap<BlockId, Vec<usize>>,
    /// What the world lays, as each is held ([`listed_as`]).
    laid: std::collections::HashSet<BlockId>,
    /// The creative-only blocks a way breaks that the world never lays (a zinc ore sample
    /// no vein sets down): ways no player meets.
    unlaid: std::collections::HashSet<BlockId>,
}

/// The item a way that takes any of `accepts` shows going in: the first a player can come
/// by, not one only the creative catalogue has (a hollow root no one can break loose).
fn shown_input(blocks: &crate::block::BlockRegistry, accepts: &[BlockId]) -> Option<BlockId> {
    let items = accepts.iter().map(|&b| blocks.item_of(b));
    items.clone().find(|&item| !blocks.def(item).has_tag(CREATIVE_ONLY)).or_else(|| items.clone().next())
}

/// The block `id` is held as: itself, or for one of its placed forms (another look, a
/// facing, upside down, against a wall) the block that form is of. A plant merged into
/// another item stays itself, as it is its own way to that item.
pub fn listed_as(blocks: &crate::block::BlockRegistry, id: BlockId) -> BlockId {
    let mut at = id;
    for _ in 0..4 {
        if !blocks.placement_form(at) {
            return at;
        }
        let def = blocks.def(at);
        let named = def.form_of().map(str::to_owned).or_else(|| def.variant_of.clone()).and_then(|name| blocks.id(&name));
        let dropped = Some(blocks.info(at).drops).filter(|&d| !d.is_air() && d != at);
        match named.or(dropped) {
            Some(base) => at = base,
            None => return at,
        }
    }
    at
}

impl RecipeBook {
    pub fn new(content: &Content) -> Self {
        let mut book = Self::default();
        let blocks = &content.blocks;
        for (index, recipe) in content.recipes.iter().enumerate() {
            // A merged-away plant in a cell is shown as the item it merged into; a cell taking
            // any of several items (`tag:toolstone`) shows each in turn, as variants do.
            let layouts: Vec<[Option<BlockId>; GRID]> = recipe.layouts().into_iter().map(|cells| cells.map(|cell| cell.map(|b| blocks.item_of(b)))).collect();
            let cells = layouts[0];
            // It is a use of everything any cell takes.
            let items: Vec<BlockId> = recipe.alternatives().into_iter().flatten().flatten().map(|&b| blocks.item_of(b)).collect();
            // The same thing from the same shape: another variant of the entry before.
            if let Some(last) = book.entries.last_mut()
                && last.output == recipe.output
                && last.count == (recipe.count, recipe.count)
                && let Method::Craft { variants, shapeless, near, ways, skill, level, .. } = &mut last.method
                && *shapeless == recipe.shapeless()
                && *near == recipe.near
                && (*skill, *level) == (recipe.skill, recipe.level)
                && variants[0].map(|c| c.is_some()) == cells.map(|c| c.is_some())
            {
                variants.extend(layouts);
                *ways += 1;
                let index = book.entries.len() - 1;
                for item in items {
                    let uses = book.uses.entry(item).or_default();
                    if uses.last() != Some(&index) {
                        uses.push(index);
                    }
                }
                continue;
            }
            let method = Method::Craft { variants: layouts, shapeless: recipe.shapeless(), first: index, ways: 1, near: recipe.near.clone(), skill: recipe.skill, level: recipe.level };
            // The blocks it is made near are among what it uses, as a cooking station is.
            let stations = method.stations(blocks);
            book.add(blocks, Entry { method, output: recipe.output, count: (recipe.count, recipe.count) }, items.into_iter().chain(stations).collect());
        }
        for (input, recipe) in content.cooking.iter() {
            let station = recipe.def.station.clone();
            let input = blocks.item_of(input);
            let with: Vec<(BlockId, u8)> = recipe.with.iter().map(|&(item, n)| (blocks.item_of(item), n)).collect();
            let method = Method::Cook { input, uses: recipe.def.uses, with: with.clone(), station, skill: recipe.skill, level: recipe.def.level, burnt: recipe.burnt };
            let uses = [input].into_iter().chain(with.iter().map(|&(item, _)| item)).chain(method.stations(blocks)).collect();
            book.add(blocks, Entry { method, output: recipe.output, count: (recipe.def.count, recipe.def.count) }, uses);
            // What is left when it burns is got this way too (the entry says it burns to
            // it), unless that is what went in (clay left too long in the kiln).
            if let Some(burnt) = recipe.burnt.filter(|&b| !blocks.merged_away(b) && blocks.item_of(b) != input) {
                let index = book.entries.len() - 1;
                book.makes.entry(blocks.item_of(burnt)).or_default().push(index);
            }
        }
        // Machines: each recipe's outputs and byproducts, worked by every machine that
        // takes its kind.
        for (id, process) in content.machines.processes() {
            let machines: Vec<BlockId> = content.machines.iter().filter(|m| m.processes.contains(&id)).map(|m| m.block).collect();
            let inputs: Vec<(BlockId, u8)> = process.inputs.iter().filter_map(|(accepts, n)| Some((shown_input(blocks, accepts)?, *n))).collect();
            let made = process.outputs.iter().map(|&(item, n)| (item, n, 1.0)).chain(process.byproducts.iter().copied());
            for (item, n, chance) in made {
                let method = Method::Process { machines: machines.clone(), inputs: inputs.clone(), tier: process.tier, chance };
                let uses = process.inputs.iter().flat_map(|(accepts, _)| accepts.iter().copied()).chain(method.stations(blocks)).collect();
                book.add(blocks, Entry { method, output: item, count: (n, n) }, uses);
            }
        }
        // Home stations: each work, left in the station's empty block.
        for station in &content.stations.stations {
            for work in &station.works {
                let Some(input) = shown_input(blocks, &work.accepts) else { continue };
                let method = Method::Work { station: station.empty, input, takes: work.takes, skill: work.skill, level: work.level };
                let uses = work.accepts.iter().copied().chain(method.stations(blocks)).collect();
                book.add(blocks, Entry { method, output: work.output, count: (work.gives, work.gives) }, uses);
            }
        }
        // Crops: what a ripe one gives, the seed going into it, and what each of its wild
        // plants gives foraged, the plant going into that.
        for (_, crop) in content.crops.iter() {
            let ripe = crop.ripe();
            let sown = crop.seed.unwrap_or(ripe);
            let wild: Vec<BlockId> = crop.def.wild.iter().filter_map(|name| blocks.id(name)).map(|plant| blocks.item_of(plant)).collect();
            let ways = [(false, sown)].into_iter().map(|(forage, from)| (forage, from, &crop.harvest)).chain(wild.into_iter().map(|plant| (true, plant, &crop.forage)));
            for (forage, from, list) in ways {
                for (item, loot) in list {
                    let method = Method::Harvest { crop: ripe, from, forage, chance: loot.chance, skill: crop.skill, level: crop.def.level };
                    let uses = if forage { vec![from] } else { crop.seed.into_iter().collect() };
                    book.add(blocks, Entry { method, output: *item, count: loot.count }, uses);
                }
            }
        }
        // Breaking a block: what it gives when that is not the block itself, or when it
        // takes a tier of tool; and what comes instead of it now and then. A block's looks,
        // facings and other placed forms read as the block, so each way is listed once.
        // Each way breaking gives, by what is broken as which tool, and the entry it is: a
        // form that gives more (a pair of plants) widens the count of the way already there.
        let mut broken: FxHashMap<_, usize> = FxHashMap::default();
        for block in blocks.ids() {
            let info = blocks.info(block);
            let rule = content.tools.rule(block);
            let tool = rule.and_then(|rule| content.tools.first_of(rule.tool?, rule.tier));
            let (skill, level) = rule.map_or((None, 1), |rule| (rule.skill, rule.level));
            let tiered = rule.is_some_and(|rule| rule.tool.is_some() && rule.tier > 0);
            // A wild plant whose item merged into another gives that item: a way to it, which
            // nothing uses (the plant is never held).
            let merged = blocks.merged_away(block);
            // A placed form reads as its block (a well facing east breaks as the well, so
            // gives no way to it); a wild plant merged away reads as itself.
            let breaks_as = listed_as(blocks, block);
            let mut once = |book: &mut RecipeBook, output: BlockId, chance: f32, count: u8| match broken.get(&(breaks_as, output, tool, skill, level, chance.to_bits())) {
                Some(&at) => {
                    let had = &mut book.entries[at].count;
                    *had = (had.0.min(count), had.1.max(count));
                    false
                }
                None => {
                    broken.insert((breaks_as, output, tool, skill, level, chance.to_bits()), book.entries.len());
                    true
                }
            };
            if !info.drops.is_air() && info.drop_count > 0 && (blocks.item_of(info.drops) != breaks_as || tiered) && once(&mut book, blocks.item_of(info.drops), 1.0, info.drop_count) {
                let method = Method::Break { block, tool, skill, level, chance: 1.0 };
                let uses = if merged { Vec::new() } else { vec![block] };
                book.add(blocks, Entry { method, output: blocks.item_of(info.drops), count: (info.drop_count, info.drop_count) }, uses);
            }
            if let Some((item, chance)) = rule.and_then(|rule| rule.instead) {
                // One that waits on a kind of tool (tall grass's twine, a knife) shows the least
                // of that kind, and each of them lists it among its uses.
                let by = rule.and_then(|rule| rule.by);
                let tools: Vec<BlockId> = by.map(|kind| blocks.ids().filter(|&b| content.tools.tool(Some(b)).kind == Some(kind)).collect()).unwrap_or_default();
                if once(&mut book, blocks.item_of(item), chance, 1) {
                    let tool = by.map_or(tool, |kind| content.tools.first_of(kind, 0));
                    let method = Method::Break { block, tool, skill, level, chance };
                    book.add(blocks, Entry { method, output: blocks.item_of(item), count: (1, 1) }, [block].into_iter().chain(tools).collect());
                }
            }
        }
        for catch in content.fishing.iter() {
            let method = Method::Catch { skill: content.fishing.skill, level: catch.def.level };
            book.add(blocks, Entry { method, output: catch.item, count: (1, 1) }, Vec::new());
        }
        for (id, creature) in content.creatures.iter() {
            for (product, &item) in creature.def.products.iter().zip(&creature.products) {
                let tools: Vec<BlockId> = product.tool.as_deref().map(|tag| content.blocks.tagged(tag).collect()).unwrap_or_default();
                let method = Method::Product { creature: id, tool: product.tool.clone() };
                book.add(blocks, Entry { method, output: item, count: product.count }, tools);
            }
            for drop in &creature.loot {
                let method = Method::Drop { creature: id, chance: drop.loot.chance };
                book.add(blocks, Entry { method, output: drop.item, count: drop.loot.count }, Vec::new());
            }
        }
        // Traps: what each creature a trap catches would drop, taken from the sprung trap.
        for trap in &content.traps.traps {
            for &creature in &trap.catches {
                for drop in &content.creatures.get(creature).loot {
                    let method = Method::Trap { trap: trap.set, creature, chance: trap.chance(creature) * drop.loot.chance, skill: trap.skill, level: trap.level };
                    let uses = method.stations(blocks);
                    book.add(blocks, Entry { method, output: drop.item, count: drop.loot.count }, uses);
                }
            }
        }
        // Bees: what each take gives, and what the held item becomes (a skep of bees).
        for take in &content.bees.takes {
            let made = take.gives.iter().map(|(item, loot)| (*item, loot.count, loot.chance)).chain(take.held_becomes.map(|item| (item, (1, 1), 1.0)));
            for (item, count, chance) in made {
                let method = Method::Bees { block: take.block, holding: take.holding, chance, skill: take.skill, level: take.level };
                let uses = method.stations(blocks).into_iter().chain(take.holding).collect();
                book.add(blocks, Entry { method, output: item, count }, uses);
            }
        }
        // Locked chests: their gold, each find, and each of a group's items at its share.
        let mut chests: Vec<_> = content.treasure.chests().collect();
        chests.sort_by_key(|(block, _)| *block);
        for (chest, held) in chests {
            let table = &content.treasure.tables[held.table];
            let gold = content.treasure.gold().filter(|_| held.gold.1 > 0).map(|coin| (coin, (held.gold.0.min(255) as u8, held.gold.1.min(255) as u8), 1.0));
            let finds = table.loot.iter().map(|r| (r.item, r.loot.count, r.loot.chance));
            let groups = table.one_of.iter().flat_map(|(chance, count, items)| items.iter().map(move |&item| (item, *count, chance / items.len() as f32)));
            for (item, count, chance) in gold.into_iter().chain(finds).chain(groups) {
                let method = Method::Chest { chest, chance };
                book.add(blocks, Entry { method, output: item, count }, vec![chest]);
            }
        }
        // Parties on the road: what their lines give (for what they ask, if anything), and
        // what a freed captive, a guide, a lure, a show or a hunt's share gives.
        for (index, event) in content.wandering.iter() {
            let lines = event.meetings.iter().flat_map(|m| &m.lines);
            for line in lines {
                let wants: Vec<Option<(crate::wander::Want, u8)>> = match (&line.wants, line.asks.is_empty()) {
                    (Some(wants), _) => vec![Some(wants.clone())],
                    (None, false) => line.asks.iter().map(|&(item, n)| Some((crate::wander::Want::Item(item), n))).collect(),
                    (None, true) => vec![None],
                };
                for wants in wants {
                    book.trade(content, index, wants, &line.gives, 1.0);
                }
            }
            for gifts in [&event.thanks, &event.guide_thanks, &event.reveal, &event.show, &event.share] {
                book.trade(content, index, None, gifts, 1.0);
            }
        }
        // Quests: what finishing each gives.
        for quest in content.quests.iter() {
            for reward in &quest.rewards {
                if let crate::quest::Reward::Item(name, count) = reward
                    && let Some(item) = blocks.id(name)
                {
                    book.add(blocks, Entry { method: Method::Reward { quest: quest.name.clone() }, output: item, count: (*count, *count) }, Vec::new());
                }
            }
        }
        // Fire: what a light set down leaves burnt out and what one is lit again into, each
        // as breaking it gives it, from what breaking the light gives (a guttering torch is
        // picked up as a torch); and what an item burning in hand leaves. What breaks to
        // nothing is no way to anything.
        let held = |b: BlockId| Some(blocks.info(b).drops).filter(|d| !d.is_air());
        let mut fire = Vec::new();
        for b in blocks.ids() {
            if let Some((_, to)) = content.flames.burns_out(blocks, b) {
                fire.push((held(b), held(to), false, None));
            }
            if let Some(to) = content.flames.relit(blocks, b) {
                fire.push((held(b), held(to), true, content.flames.relit_with(b)));
            }
            if let Some(to) = content.tools.burnt(b) {
                fire.push((Some(b), held(to), false, None));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for (from, output, relit, fuel) in fire {
            if let (Some(from), Some(output)) = (from, output)
                && from != output
                && seen.insert((from, output, relit))
            {
                // The fuel it uses up is among what it takes, so fat looked up shows the lamp.
                let takes = std::iter::once(from).chain(fuel.into_iter().flat_map(|tag| blocks.tagged(tag))).collect();
                book.add(blocks, Entry { method: Method::Burn { from, relit, fuel: fuel.map(str::to_owned) }, output, count: (1, 1) }, takes);
            }
        }
        // A smouldering structure: what each of its wood cells, charred through or opened
        // early, breaks to, as many as the heap drops.
        for structure in content.multiblocks.iter() {
            let Some(smoulder) = &structure.smoulder else { continue };
            let Some(&from) = smoulder.wood.first().and_then(|&at| structure.takes(at).first()) else { continue };
            for (left, early) in [(smoulder.done, false), (smoulder.early, true)] {
                if let Some(output) = held(left) {
                    let n = blocks.info(left).drop_count;
                    book.add(blocks, Entry { method: Method::Smoulder { structure: structure.name.clone(), from, early }, output, count: (n, n) }, vec![from]);
                }
            }
        }
        book
    }

    /// Adds the items a party gives, for `wants` if it asks for something, `chance` of the
    /// time (a pick of one of several, a share of it).
    fn trade(&mut self, content: &Content, event: usize, wants: Option<(crate::wander::Want, u8)>, gives: &[crate::wander::ResolvedReward], chance: f32) {
        use crate::wander::{ResolvedReward, Want};
        for reward in gives {
            match reward {
                ResolvedReward::Item(item, count) => {
                    let uses = match &wants {
                        Some((Want::Item(asked), _)) => vec![*asked],
                        Some((Want::Tag(tag), _)) => content.blocks.tagged(tag).collect(),
                        None => Vec::new(),
                    };
                    let method = Method::Trade { event, wants: wants.clone(), chance };
                    self.add(&content.blocks, Entry { method, output: *item, count: *count }, uses);
                }
                ResolvedReward::OneOf(choices) => self.trade(content, event, wants.clone(), choices, chance / choices.len().max(1) as f32),
                _ => {}
            }
        }
    }

    /// Whether the world lays `block` (as [`found`](Self::found) was told): a creative-only
    /// block a player only ever meets where the generator set it.
    pub fn laid(&self, blocks: &crate::block::BlockRegistry, block: BlockId) -> bool {
        self.laid.contains(&listed_as(blocks, block))
    }

    /// Whether a player can meet `entry`: not by breaking a creative-only block the world
    /// never lays (as [`found`](Self::found) was told).
    pub fn met(&self, entry: &Entry) -> bool {
        !matches!(entry.method, Method::Break { block, .. } if self.unlaid.contains(&block))
    }

    /// Adds what the world lays (`placed`, from its generator's data) that breaking gives
    /// back as itself: found, not made. Items that are never placed (seeds) are left out,
    /// and so are furnishings: one the world placed (a village's bed) gives nothing broken.
    pub fn found(&mut self, content: &Content, placed: impl IntoIterator<Item = BlockId>) {
        let blocks = &content.blocks;
        let mut placed: Vec<BlockId> = placed.into_iter().filter(|&b| !blocks.def(b).has_tag("furnishing")).collect();
        placed.sort();
        placed.dedup();
        self.laid.extend(placed.iter().map(|&b| listed_as(blocks, b)));
        let laid = &self.laid;
        self.unlaid = self
            .entries
            .iter()
            .filter_map(|e| match e.method {
                Method::Break { block, .. } if blocks.def(block).has_tag(CREATIVE_ONLY) && !laid.contains(&listed_as(blocks, block)) => Some(block),
                _ => None,
            })
            .collect();
        // A crop the world lays ripe (a wild cabbage) is foraged as it stands.
        for (_, crop) in content.crops.iter().filter(|(_, crop)| placed.contains(&crop.ripe())) {
            for (item, loot) in &crop.forage {
                let method = Method::Harvest { crop: crop.ripe(), from: crop.ripe(), forage: true, chance: loot.chance, skill: crop.skill, level: crop.def.level };
                self.add(blocks, Entry { method, output: *item, count: loot.count }, vec![crop.ripe()]);
            }
        }
        for block in placed {
            let info = blocks.info(block);
            let broken_whole = !blocks.def(block).item && info.drop_count > 0 && blocks.item_of(info.drops) == block && blocks.def(block).hardness >= 0.0;
            // A block a tool must take whole already says so (`Break` with its tool).
            let said = self.making(block).iter().any(|e| matches!(e.method, Method::Break { block: b, .. } if b == block));
            if !broken_whole || said || blocks.merged_away(block) {
                continue;
            }
            let rule = content.tools.rule(block);
            let tool = rule.and_then(|rule| content.tools.first_of(rule.tool?, rule.tier));
            let (skill, level) = rule.map_or((None, 1), |rule| (rule.skill, rule.level));
            self.add(blocks, Entry { method: Method::Found { tool, skill, level }, output: block, count: (info.drop_count, info.drop_count) }, Vec::new());
        }
    }

    /// Every item a player could hold that the book has no way to get and that is not
    /// tagged [`CREATIVE_ONLY`]: a dead entry, which content refuses to load with. Breaking
    /// the item's own block counts only where the world lays it. And the
    /// items so tagged that the book does have a way to, whose mark is stale.
    pub fn unsourced(&self, blocks: &crate::block::BlockRegistry) -> (Vec<BlockId>, Vec<BlockId>) {
        let (mut dead, mut stale) = (Vec::new(), Vec::new());
        for id in blocks.ids().filter(|&id| blocks.holdable(id)) {
            // Breaking an item's own block to get it is no way in, unless the world lays it.
            let circular = |e: &&Entry| matches!(e.method, Method::Break { block, .. } if listed_as(blocks, block) == e.output && !self.laid.contains(&e.output));
            let ways = self.making(id).into_iter().filter(|e| !circular(e)).count();
            match (ways == 0, blocks.def(id).has_tag(CREATIVE_ONLY)) {
                (true, false) => dead.push(id),
                (false, true) => stale.push(id),
                _ => {}
            }
        }
        (dead, stale)
    }

    /// Adds an entry, unless it makes a plant whose item merged into another; one using such
    /// a plant lists the item it merged into (`merged` blocks are never held).
    fn add(&mut self, blocks: &crate::block::BlockRegistry, entry: Entry, mut uses: Vec<BlockId>) {
        if blocks.merged_away(entry.output) {
            return;
        }
        uses.iter_mut().for_each(|b| *b = blocks.item_of(*b));
        let index = self.entries.len();
        self.makes.entry(entry.output).or_default().push(index);
        uses.sort();
        uses.dedup();
        for item in uses {
            self.uses.entry(item).or_default().push(index);
        }
        self.entries.push(entry);
    }

    /// Every entry, in the order the content gives them (crafting, cooking, machines, home
    /// stations, crops, breaking, fishing, creatures, traps, bees, chests, parties on the
    /// road, quests, then what is found in the world).
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Every way to get `item`.
    pub fn making(&self, item: BlockId) -> Vec<&Entry> {
        self.makes.get(&item).map_or_else(Vec::new, |found| found.iter().map(|&i| &self.entries[i]).collect())
    }

    /// Everything `item` goes into: as an ingredient, a station or a tool.
    pub fn using(&self, item: BlockId) -> Vec<&Entry> {
        self.uses.get(&item).map_or_else(Vec::new, |found| found.iter().map(|&i| &self.entries[i]).collect())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> Content {
        Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads")
    }

    #[test]
    fn every_guide_icon_is_an_item() {
        let content = content();
        for id in content.skills.ids() {
            let skill = content.skills.get(id);
            for step in &skill.guide {
                if let Some(icon) = &step.icon {
                    let found = content.blocks.id(icon).or_else(|| content.blocks.id(&format!("base:{icon}")));
                    assert!(found.is_some(), "{}'s guide line {:?} names `{icon}`, which is no item", skill.name, step.text);
                }
            }
        }
    }

    #[test]
    fn plants_whose_item_merged_are_not_in_the_book() {
        let content = content();
        let blocks = &content.blocks;
        let (oregano, herbs, stone) = (blocks.id("base:oregano").unwrap(), blocks.id("base:kitchen_herbs").unwrap(), blocks.id("base:stone").unwrap());
        assert!(blocks.merged_away(oregano) && !blocks.merged_away(herbs) && !blocks.listed(oregano) && blocks.listed(herbs));
        assert_eq!(blocks.item_of(oregano), herbs);
        let mut book = RecipeBook::default();
        // An entry making one is dropped, and one using it lists the item it merged into.
        book.add(blocks, Entry { method: Method::Drop { creature: CreatureId(0), chance: 1.0 }, output: oregano, count: (1, 1) }, Vec::new());
        book.add(blocks, Entry { method: Method::Drop { creature: CreatureId(0), chance: 1.0 }, output: stone, count: (1, 1) }, vec![oregano, herbs]);
        assert!(book.making(oregano).is_empty() && book.using(oregano).is_empty(), "a merged-away plant is in the book");
        assert_eq!((book.making(stone).len(), book.using(herbs).len()), (1, 1));
        // And the game's own book never mentions one.
        let book = RecipeBook::new(&content);
        assert!(blocks.ids().filter(|&id| blocks.merged_away(id)).all(|id| book.making(id).is_empty() && book.using(id).is_empty()));
    }

    /// What breaking gives in place of a block (gravel's flint, tall grass's twine with a
    /// knife) is in the book, with the block and the tool it waits on, and each knife lists
    /// twine among its uses.
    #[test]
    fn what_breaking_gives_is_in_the_book() {
        let content = content();
        let book = RecipeBook::new(&content);
        let id = |name: &str| content.blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
        let twine = book.making(id("base:twine"));
        assert!(
            twine.iter().any(|e| matches!(e.method, Method::Break { block, tool: Some(tool), chance, .. } if block == id("base:tall_grass") && tool == id("base:flint_knife") && chance > 0.0)),
            "{twine:?}"
        );
        assert!(book.making(id("base:flint")).iter().any(|e| matches!(e.method, Method::Break { block, tool: None, .. } if block == id("base:gravel"))));
        for knife in ["base:flint_knife", "base:stone_knife", "base:iron_knife"] {
            assert!(book.using(id(knife)).iter().any(|e| e.output == id("base:twine")), "{knife} lists no twine");
        }
    }

    #[test]
    fn the_game_content_reads_both_ways() {
        let content = content();
        let book = RecipeBook::new(&content);
        let id = |name: &str| content.blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
        // Planks are crafted from a log; the log goes into planks and the campfire.
        let planks = book.making(id("base:oak_planks"));
        assert!(planks.iter().any(|e| matches!(&e.method, Method::Craft { variants, .. } if variants[0].contains(&Some(id("base:oak_log"))))), "{planks:?}");
        // A campfire from any wood is one entry, its woods its variants.
        let campfires = book.making(id("base:campfire"));
        assert_eq!(campfires.len(), 1, "{campfires:?}");
        let log_uses: Vec<BlockId> = book.using(id("base:oak_log")).iter().map(|e| e.output).collect();
        assert!(log_uses.contains(&id("base:oak_planks")) && log_uses.contains(&id("base:campfire")), "{log_uses:?}");
        // The campfire is used as a station; cooked food is made there.
        assert!(book.using(id("base:campfire")).iter().any(|e| matches!(e.method, Method::Cook { .. })));
        // Shears are used on a creature for its product.
        assert!(book.using(id("base:shears")).iter().any(|e| matches!(&e.method, Method::Product { tool: Some(tool), .. } if tool == "shears")));
    }

    /// A cell taking a tag (`tag:toolstone`) is a use of every item it takes, and the card
    /// shows each a player can hold in turn: never the natural rock that breaks into them.
    #[test]
    fn a_tag_cell_is_a_use_of_each_item_it_takes() {
        let content = content();
        let book = RecipeBook::new(&content);
        let id = |name: &str| content.blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
        let head = id("base:stone_head");
        for cobbles in ["base:cobbled_granite", "base:cobbled_diorite", "base:cobbled_andesite", "base:cobbled_basalt", "base:cobbled_deepslate"] {
            assert!(book.using(id(cobbles)).iter().any(|e| e.output == head), "{cobbles} goes into no stone head");
        }
        let [entry] = book.making(head)[..] else { panic!("one way to a stone head: {:?}", book.making(head)) };
        let Method::Craft { variants, ways, .. } = &entry.method else { panic!("{entry:?}") };
        let shown: Vec<BlockId> = variants.iter().flat_map(|v| v.iter().flatten().copied()).collect();
        assert_eq!(*ways, 1);
        assert!(shown.contains(&id("base:cobbled_granite")) && shown.contains(&id("base:cobbled_deepslate")), "{shown:?}");
        assert!(!shown.contains(&id("base:deepslate")) && !shown.contains(&id("base:granite")), "natural rock on the card: {shown:?}");
        // And what a cell wants, short, is one a player can hold.
        let recipe = content.recipes.iter().find(|r| r.output == head).unwrap();
        assert!(!content.blocks.def(recipe.cells().into_iter().flatten().next().unwrap()).has_tag("natural"));
        assert!(recipe.short(&Default::default()).iter().all(|&(item, _)| !content.blocks.def(item).has_tag("natural")));
    }

    #[test]
    fn every_way_the_game_makes_things_is_in_the_book() {
        let content = content();
        let book = RecipeBook::new(&content);
        let id = |name: &str| content.blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
        let ways = |item: &str| book.making(id(item)).into_iter().map(|e| e.method.clone()).collect::<Vec<_>>();
        // A machine: iron dust is crushed from iron ore, which goes into the crusher's way.
        assert!(ways("base:iron_dust").iter().any(|m| matches!(m, Method::Process { inputs, machines, .. } if inputs.contains(&(id("base:iron_ore"), 1)) && !machines.is_empty())), "{:?}", ways("base:iron_dust"));
        assert!(book.using(id("base:iron_ore")).iter().any(|e| e.output == id("base:iron_dust")));
        // A home station: jerky dries from raw meat, the station among its uses.
        let jerky = ways("base:jerky");
        let Some(Method::Work { station, skill, .. }) = jerky.iter().find(|m| matches!(m, Method::Work { .. })) else { panic!("{jerky:?}") };
        assert!(book.using(*station).iter().any(|e| e.output == id("base:jerky")) && content.skills.get(*skill).name == "core:cooking");
        // A crop's harvest, its seed going into it.
        let wheat = ways("base:wheat");
        assert!(wheat.iter().any(|m| matches!(m, Method::Harvest { forage: false, .. })), "{wheat:?}");
        // Foraging: a carrot from a wild carrot, which is what goes in, not the carrot.
        let carrot = ways("base:carrot");
        assert!(carrot.iter().any(|m| matches!(m, Method::Harvest { forage: true, from, .. } if *from == id("base:wild_carrot"))), "{carrot:?}");
        assert!(book.using(id("base:wild_carrot")).iter().any(|e| e.output == id("base:carrot")));
        // No way is listed twice: a block's looks and facings break as the block does (a
        // wild plant merged away, as itself). And none is breaking a block to get that
        // block back without a tool to take it whole (a well facing east gives no well).
        let breaks_as = |b: BlockId| listed_as(&content.blocks, b);
        assert_eq!(breaks_as(id("base:well_east")), id("base:well"), "a well's facing reads as the well");
        for entry in book.entries() {
            if let Method::Break { block, tool: None, .. } = entry.method {
                assert_ne!(breaks_as(block), entry.output, "breaking {} gives itself", content.blocks.def(block).name);
            }
        }
        for item in content.blocks.ids() {
            let made = book.making(item);
            for (i, entry) in made.iter().enumerate() {
                if let Method::Break { block, tool, chance, .. } = &entry.method {
                    let same = |other: &Entry| matches!(&other.method, Method::Break { block: b, tool: t, chance: c, .. } if breaks_as(*b) == breaks_as(*block) && t == tool && c == chance);
                    assert!(!made[..i].iter().any(|other| same(other)), "{} lists breaking {} twice", content.blocks.def(item).name, content.blocks.def(*block).name);
                }
            }
        }
        // Breaking: stone gives its loose stones always, with a tool; gravel gives flint an
        // eighth of the time, instead of itself.
        assert!(ways("base:loose_stone").iter().any(|m| matches!(m, Method::Break { block, chance, .. } if *block == id("base:stone") && *chance == 1.0)));
        assert!(ways("base:flint").iter().any(|m| matches!(m, Method::Break { block, chance, .. } if *block == id("base:gravel") && *chance == 0.125)));
        assert!(book.using(id("base:gravel")).iter().any(|e| e.output == id("base:flint")));
        // Every catch is caught on a line, at its level.
        for catch in content.fishing.iter() {
            assert!(book.making(catch.item).iter().any(|e| e.method == Method::Catch { skill: content.fishing.skill, level: catch.def.level }), "{}", catch.def.item);
        }
        // A crafting recipe that takes a level says so.
        let levelled = content.recipes.iter().find(|r| r.skill.is_some() && r.level > 1).expect("a recipe takes a level");
        assert!(book.making(levelled.output).iter().any(|e| matches!(e.method, Method::Craft { skill, level, .. } if skill == levelled.skill && level == levelled.level)));
    }

    #[test]
    fn traps_bees_chests_parties_and_quests_are_ways_too() {
        let content = content();
        let book = RecipeBook::new(&content);
        let id = |name: &str| content.blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
        let ways = |item: &str| book.making(id(item)).into_iter().map(|e| e.method.clone()).collect::<Vec<_>>();
        // A snare's catch: a rabbit's loot, taken at the trap's level; the snare is among
        // what the way uses.
        let snare = id("base:snare");
        let rabbit = content.creatures.id("base:rabbit").unwrap();
        let caught: Vec<BlockId> = book.using(snare).iter().filter(|e| matches!(e.method, Method::Trap { creature, .. } if creature == rabbit)).map(|e| e.output).collect();
        assert_eq!(caught.len(), content.creatures.get(rabbit).loot.len(), "{caught:?}");
        // A hare is held by its own chance, not the snare's: what it leaves comes that much
        // less often.
        let hare = content.creatures.id("base:hare").unwrap();
        let trap = content.traps.traps.iter().find(|t| t.set == snare).unwrap();
        let held = book.using(snare).into_iter().filter_map(|e| match e.method { Method::Trap { creature, chance, .. } if creature == hare => Some(chance), _ => None });
        let loot = content.creatures.get(hare).loot.iter().map(|d| trap.chance(hare) * d.loot.chance);
        assert!(trap.chance(hare) < 0.6 && held.eq(loot), "the hare's own chance");
        // Bees: honey from a full skep, and a skep of bees from a nest with a skep in hand.
        assert!(ways("base:honey").iter().any(|m| matches!(m, Method::Bees { block, holding: None, .. } if *block == id("base:honey_skep"))));
        assert!(ways("base:bee_skep").iter().any(|m| matches!(m, Method::Bees { block, holding: Some(skep), .. } if *block == id("base:bee_nest") && *skep == id("base:skep"))));
        // A chest's rarest find, at its chance, and its gold.
        let amulet = ways("base:grave_ward_amulet");
        assert!(amulet.iter().any(|m| matches!(m, Method::Chest { chance, .. } if *chance > 0.0 && *chance < 0.05)), "{amulet:?}");
        let gold = content.treasure.gold().expect("chests hold gold");
        assert!(book.making(gold).iter().any(|e| matches!(e.method, Method::Chest { .. })));
        // A party's swap: twine for two hides, the hides among its uses.
        let twine = book.making(id("base:twine"));
        let swap = twine.iter().find(|e| matches!(&e.method, Method::Trade { wants: Some((crate::wander::Want::Item(hide), 2)), .. } if *hide == id("base:hide"))).expect("twine for hides");
        assert!(book.using(id("base:hide")).contains(swap));
        // A quest's reward.
        let torch_quests: Vec<&str> = book.making(id("base:torch")).iter().filter_map(|e| if let Method::Reward { quest } = &e.method { Some(quest.as_str()) } else { None }).collect();
        assert!(content.quests.iter().filter(|q| q.rewards.contains(&crate::quest::Reward::Item("base:torch".into(), 4))).all(|q| torch_quests.contains(&q.name.as_str())) && !torch_quests.is_empty());
        // Fire: a torch burnt out leaves its stub, a firebrand burnt in hand a charred stick,
        // and an empty lamp filled is a lit one; a guttering torch, picked up as a torch, is
        // no way to itself.
        let burnt = |item: &str| ways(item).into_iter().filter_map(|m| if let Method::Burn { from, relit, .. } = m { Some((from, relit)) } else { None }).collect::<Vec<_>>();
        assert_eq!(burnt("base:torch_stub"), [(id("base:torch"), false)]);
        assert_eq!(burnt("base:charred_stick"), [(id("base:firebrand"), false)]);
        assert_eq!(burnt("base:clay_oil_lamp"), [(id("base:clay_oil_lamp_empty"), true)]);
        assert!(burnt("base:torch_guttering").is_empty());
        // A clamp's firewood, charred through, is charcoal; opened early, charred wood.
        let charred = |item: &str| ways(item).into_iter().filter_map(|m| if let Method::Smoulder { structure, from, early } = m { Some((structure, from, early)) } else { None }).collect::<Vec<_>>();
        assert_eq!(charred("base:charcoal"), [("base:charcoal_clamp".to_owned(), id("base:firewood"), false)]);
        assert_eq!(charred("base:charred_wood"), [("base:charcoal_clamp".to_owned(), id("base:firewood"), true)]);
        // As many as a heap breaks to: four charcoal, two charred wood.
        let heap = |item: &str| book.making(id(item)).iter().find(|e| matches!(e.method, Method::Smoulder { .. })).map(|e| e.count);
        assert_eq!((heap("base:charcoal"), heap("base:charred_wood")), (Some((4, 4)), Some((2, 2))));
        let fat = content.blocks.tagged("fat").next().expect("something is fat");
        assert!(book.using(fat).iter().any(|e| matches!(&e.method, Method::Burn { relit: true, fuel: Some(tag), .. } if tag == "fat") && e.output == id("base:clay_oil_lamp")));
        // Food left too long: the cooking entries it burns in make it.
        let burnt = book.making(id("base:burnt_food"));
        assert!(!burnt.is_empty() && burnt.iter().all(|e| matches!(e.method, Method::Cook { burnt: Some(b), .. } if b == id("base:burnt_food"))));
        // A wild plant whose item merged gives that item when broken, and is used for nothing.
        let oregano = id("base:oregano");
        assert!(ways("base:kitchen_herbs").iter().any(|m| matches!(m, Method::Break { block, .. } if *block == oregano)));
        assert!(book.using(id("base:kitchen_herbs")).iter().all(|e| e.output != id("base:kitchen_herbs")));
    }

    /// What the world lays and breaking gives back whole is found; what breaks into another
    /// thing, or is never placed, is not.
    #[test]
    fn what_the_world_lays_is_found() {
        let content = content();
        let id = |name: &str| content.blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
        let mut book = RecipeBook::new(&content);
        let (dirt, stone, seeds) = (id("base:dirt"), id("base:stone"), id("base:wheat_seeds"));
        book.found(&content, [dirt, stone, seeds, dirt]);
        let found = |item| book.making(item).iter().filter(|e| matches!(e.method, Method::Found { .. })).count();
        assert_eq!((found(dirt), found(stone), found(seeds)), (1, 0, 0), "dirt is found, stone breaks to cobblestone, seeds are never laid");
    }

    /// A station a package adds (a quern, tagged `millstone` apart from base's own quern)
    /// shows its ways, cooked at it and crafted near it, with no code for it: one path from
    /// data for every station.
    #[cfg(unix)]
    #[test]
    fn a_station_a_package_adds_lists_its_ways_with_no_code() {
        let shipped = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let dir = std::env::temp_dir().join(format!("voxmar-quern-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("mill")).unwrap();
        for package in ["base", "core"] {
            std::os::unix::fs::symlink(shipped.join(package).canonicalize().unwrap(), dir.join(package)).unwrap();
        }
        std::fs::write(dir.join("mill/blocks.ron"), r#"(blocks: [
            (name: "quern", textures: All("stone"), hardness: 1.0, color: (120, 120, 120), tags: ["millstone"]),
            (name: "meal", item: true, render: Plant, solid: false, textures: All("wheat"), hardness: 0.0, color: (220, 210, 170)),
            (name: "cake", item: true, render: Plant, solid: false, textures: All("bread"), hardness: 0.0, color: (220, 190, 130)),
        ])"#).unwrap();
        std::fs::write(dir.join("mill/cooking.ron"), r#"(recipes: [(input: "base:wheat", output: "meal", station: "millstone", skill: "core:cooking", level: 1, xp: 1.0, stop_burning: 0)])"#).unwrap();
        std::fs::write(dir.join("mill/recipes.ron"), r#"(recipes: [(ingredients: ["meal", "base:honey"], output: "cake", near: "millstone")])"#).unwrap();
        let content = Content::load(&dir).expect("the mill loads");
        std::fs::remove_dir_all(&dir).unwrap();
        let book = RecipeBook::new(&content);
        let id = |name: &str| content.blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
        let quern = id("mill:quern");
        let at: Vec<BlockId> = book.using(quern).iter().map(|e| e.output).collect();
        assert!(at.contains(&id("mill:meal")) && at.contains(&id("mill:cake")), "{at:?}");
        for item in ["mill:meal", "mill:cake"] {
            assert!(book.making(id(item)).iter().all(|e| e.method.stations(&content.blocks) == vec![quern]), "{item}");
        }
    }

    #[test]
    fn quests_asking_for_an_item_are_found_by_name_and_tag() {
        let content = content();
        let id = |name: &str| content.blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
        let asking = |item: &str| crate::quest::asking(&content.quests, &content.blocks, id(item)).into_iter().map(|q| q.name.clone()).collect::<Vec<_>>();
        // A log is asked for by tag (`Have("tag:log", 8)`), a stone pickaxe by name.
        let logs = asking("base:oak_log");
        assert!(content.quests.iter().filter(|q| logs.contains(&q.name)).any(|q| q.tasks.iter().flatten().any(|g| g.item() == Some("tag:log"))), "{logs:?}");
        assert!(!asking("base:stone_pickaxe").is_empty());
        // Nothing asks for air, and a goal for `any` names nothing.
        assert!(crate::quest::asking(&content.quests, &content.blocks, BlockId::AIR).is_empty());
    }
}
