//! Actions: the only way the world changes in discrete steps.
//!
//! Every action goes through one lifecycle, whoever performs it (player, machine, NPC):
//! propose, validate, schedule one completion, then commit atomically or cancel. Action
//! definitions are data; what they may require and do is a small, fixed vocabulary of
//! requirements and effects owned by the engine, which is what lets any package add
//! actions while the engine keeps every change validated, deterministic and networked.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::{BlockId, BlockInfo, BlockRegistry, Facing, RegistryError, Support};
use crate::coords::Face;
use crate::crop::CropRegistry;
use crate::skill::SkillId;
use crate::glam::{IVec3, Vec3};
use crate::physics::player::REACH;

/// How far a fishing line is cast, from the eye.
pub const CAST: f32 = 9.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActionId(pub u16);

/// What an action is aimed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionTarget {
    None,
    Block { block: IVec3, face: Face },
    /// Inventory slots, [`crate::inventory::CURSOR`] for the stack on the pointer, and
    /// how many items to move.
    Slots { from: u8, to: u8, count: u8 },
    Slot(u8),
    /// Up to `count` items of the stack in one slot ([`crate::inventory::CURSOR`] for the
    /// pointer's).
    Stack { slot: u8, count: u8 },
    /// A creature, by its id.
    Creature(u32),
    /// A crafting way to lay into the grid (`Effect::LayRecipe`): the recipes from `first`
    /// (in content order) for `ways` on, one recipe book entry's variants (another wood's
    /// planks), and how many sets.
    Recipe { first: u16, ways: u8, count: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Duration {
    Instant,
    Seconds(f32),
    /// The target block's hardness, in seconds.
    TargetHardness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Requirement {
    Alive,
    Dead,
    /// The target block is within the actor's reach.
    InReach,
    /// The target block exists and can be broken.
    Breakable,
    /// The actor holds a placeable block.
    HoldingBlock,
    /// The cell the held block would go into (see [`placement_cell`]) is free: air, a
    /// fluid or a plant.
    Replaceable,
    /// No agent stands where the held block would go.
    Unobstructed,
    /// A held block that needs support would have it: a plant solid ground beneath, a
    /// hanging plant a solid block above.
    Supported,
    /// The actor holds a crop's seed.
    HoldingSeed,
    /// The held seed's crop can go on top of the target: ground it grows in, with room
    /// above.
    Plantable,
    /// The actor's level in the held seed's crop's skill is at least what it needs.
    SkillLevel,
    /// The target is a ripe crop that fruits again when picked.
    Ripe,
    /// The target creature is alive and within the actor's reach with what they hold (to
    /// strike it).
    CreatureInReach,
    /// The target creature is alive and within the actor's touch, whatever they hold (to
    /// tend, look closely at or let go of it: `Feel::touch`).
    CreatureInTouch,
    /// The target creature takes what the actor holds: food that tames it or readies it
    /// to breed, or a tool that takes its product.
    CreatureAccepts,
    /// The target creature is one the actor keeps.
    CreatureIsMine,
    /// The target block is a station that cooks what the actor holds.
    Cookable,
    /// The actor's Cooking level is at least what the held item's recipe there needs.
    CookingLevel,
    /// The actor carries the tool the held item's recipe there asks for, if any (an axe to
    /// split a log on a chopping block).
    CookingTool,
    /// The actor holds something it can eat.
    HoldingFood,
    /// The actor has health to heal.
    Hurt,
    /// The actor holds the kind and tier of tool the target block asks for, if any
    /// (`tools.ron`).
    ToolGood,
    /// The actor's level in the skill the target block asks for is at least what it asks.
    BlockLevel,
    /// A ripe crop (or fruit) at the target asks for its Farming level to harvest, as an ore
    /// asks for its Mining level; anything else passes.
    CropLevel,
    /// The target is soil a hoe can till, with nothing standing on it.
    Tillable,
    /// The target is a crafting table.
    CraftingTable,
    /// The target is somewhere to sleep (tagged `bed`): a sleeping bag.
    Bed,
    /// The target is a seat (tagged `seat`: a stool, chair, bench or cushion) with room to
    /// sit on it, and the actor holds no block to place against it.
    Seat,
    /// The target is coated in place by what the actor holds (`coat` on its block), aimed
    /// at one of its sides: clay on a wattle panel or an earth wall.
    Coats,
    /// What the target is coated into, if solid, would shut nobody in: no player and no
    /// creature stands in its cell (a thin wattle panel turned a whole daub block), as
    /// placing a block and shutting a door refuse.
    CoatClear,
    /// The target is the core of a multiblock (tagged `multiblock_core`): a hearthstone,
    /// a crucible; for a machine's core, the actor holds no block to place against it.
    MultiblockCore,
    /// The target is a machine (`machines.ron`), and the actor holds nothing, something it
    /// takes, or something that is not a block to place (a placeable block it does not
    /// take is placed against it instead).
    Machine,
    /// The target is an altar (tagged `altar`): a stone altar, a shrine's.
    Altar,
    /// The actor holds a fishing rod and the target is water with open air above, near
    /// enough to cast to.
    Fishable,
    /// The target is a locked chest (tagged `locked`, `treasure.ron`).
    Locked,
    /// Something can be taken from the target's bees with what the actor holds (a swarm
    /// into a skep, honey from a full one: `bees.ron`).
    Bees,
    /// The target is a home station (`stations.ron`): an empty one with something it works
    /// in hand, or one working or ready.
    Station,
    /// The target is a sprung trap (`traps.ron`), with what it caught to take.
    SprungTrap,
    /// The actor holds a fertiliser and the target is a crop still growing
    /// (`stations.ron`'s `fertilisers`).
    Fertilisable,
    /// The target is a store (a bin that does not feed: a chest, a cupboard, a jar), and
    /// the actor holds no machine to lay against it.
    Store,
    /// The target turns into another block when used (`swaps`: a door), and nobody stands
    /// where it would shut.
    Swaps,
    /// The target is a light gone out that is lit again (`relights_to:`, a torch's stub),
    /// and the actor holds something that `kindles` (`flame`).
    Relights,
}

impl Requirement {
    /// Every requirement, in [`Requirement::index`] order (`refusal` says why each failed).
    pub const ALL: [Requirement; 42] = [Requirement::Alive, Requirement::Dead, Requirement::InReach, Requirement::Breakable, Requirement::HoldingBlock, Requirement::Replaceable, Requirement::Unobstructed, Requirement::Supported, Requirement::HoldingSeed, Requirement::Plantable, Requirement::SkillLevel, Requirement::Ripe, Requirement::CreatureInReach, Requirement::CreatureInTouch, Requirement::CreatureAccepts, Requirement::CreatureIsMine, Requirement::Cookable, Requirement::CookingLevel, Requirement::HoldingFood, Requirement::Hurt, Requirement::ToolGood, Requirement::BlockLevel, Requirement::CropLevel, Requirement::Tillable, Requirement::CraftingTable, Requirement::Bed, Requirement::MultiblockCore, Requirement::Machine, Requirement::Altar, Requirement::Fishable, Requirement::Locked, Requirement::Bees, Requirement::Station, Requirement::Fertilisable, Requirement::Store, Requirement::Swaps, Requirement::SprungTrap, Requirement::Seat, Requirement::Coats, Requirement::CookingTool, Requirement::CoatClear, Requirement::Relights];

    /// Where it stands in [`Requirement::ALL`]. A new requirement must be added to both:
    /// this match does not compile without it.
    pub fn index(self) -> usize {
        match self {
            Requirement::Alive => 0,
            Requirement::Dead => 1,
            Requirement::InReach => 2,
            Requirement::Breakable => 3,
            Requirement::HoldingBlock => 4,
            Requirement::Replaceable => 5,
            Requirement::Unobstructed => 6,
            Requirement::Supported => 7,
            Requirement::HoldingSeed => 8,
            Requirement::Plantable => 9,
            Requirement::SkillLevel => 10,
            Requirement::Ripe => 11,
            Requirement::CreatureInReach => 12,
            Requirement::CreatureInTouch => 13,
            Requirement::CreatureAccepts => 14,
            Requirement::CreatureIsMine => 15,
            Requirement::Cookable => 16,
            Requirement::CookingLevel => 17,
            Requirement::HoldingFood => 18,
            Requirement::Hurt => 19,
            Requirement::ToolGood => 20,
            Requirement::BlockLevel => 21,
            Requirement::CropLevel => 22,
            Requirement::Tillable => 23,
            Requirement::CraftingTable => 24,
            Requirement::Bed => 25,
            Requirement::MultiblockCore => 26,
            Requirement::Machine => 27,
            Requirement::Altar => 28,
            Requirement::Fishable => 29,
            Requirement::Locked => 30,
            Requirement::Bees => 31,
            Requirement::Station => 32,
            Requirement::Fertilisable => 33,
            Requirement::Store => 34,
            Requirement::Swaps => 35,
            Requirement::SprungTrap => 36,
            Requirement::Seat => 37,
            Requirement::Coats => 38,
            Requirement::CookingTool => 39,
            Requirement::CoatClear => 40,
            Requirement::Relights => 41,
        }
    }

    /// Its name, as `actions.ron` writes it: `ToolGood`.
    pub fn name(self) -> String {
        ron::to_string(&self).unwrap_or_default()
    }

    /// The requirement called `name`, as `actions.ron` writes it.
    pub fn named(name: &str) -> Option<Requirement> {
        Requirement::ALL.into_iter().find(|need| need.name() == name)
    }
}

/// Engine-owned effects that actions compile down to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    /// Give the actor what the target block drops, straight into its inventory.
    GiveDrop,
    /// Pop what the target block and the plants it holds up drop out into the world, as
    /// dropped items (see [`crate::dropped`]).
    DropLoot,
    /// Throw the target stack's items from the actor's hand into the world.
    Throw,
    /// Turn the target block into air, and the plant standing on it if there is one.
    ClearTarget,
    /// Take one held item and put it into the placement cell of the target.
    PlaceHeld,
    /// Move items between the actor's inventory slots (see
    /// [`crate::inventory::Inventory::move_items`]).
    MoveItems,
    /// Move a stack between the hotbar and the rest of the inventory.
    QuickMove,
    /// Put the stack on the pointer back into the inventory.
    StowCursor,
    /// Strike the target creature with what the actor holds.
    Strike,
    /// A dodge: a moment no blow lands (`combat`); the client dashes.
    Dodge,
    /// Cook one held item at the target station: its recipe's food, or burnt.
    Cook,
    /// Tend the target creature with what the actor holds: feed it (taming it, or readying
    /// a kept one to breed) or take its product with a tool.
    Tend,
    /// Look closely at the target creature: what the actor learns grows with Farming.
    Inspect,
    /// Let the target creature, one the actor keeps, go wild again (asked twice).
    Release,
    /// Restore the actor's health and return it to spawn.
    Respawn,
    /// Sow one held seed on top of the target, and grant the crop's planting experience.
    Plant,
    /// Pick a ripe crop's harvest, leaving the plant to fruit again, and grant the crop's
    /// harvest experience.
    Pick,
    /// Eat one held item, healing the actor by its food value (less for the same food again
    /// and again, see `vitals::Recent`).
    EatHeld,
    /// Craft what the actor's crafting grid holds: once onto the pointer
    /// (`ActionTarget::Slot(1)`), or as many times as it can into the inventory (`Slot(0)`).
    Craft,
    /// Lay a recipe for the target item into the actor's empty crafting grid, `count` sets
    /// of it, from their pack and, at home, from the stores in it (`craft::lay`).
    LayRecipe,
    /// Till the target soil into farmland.
    Till,
    /// Wear what the actor holds by a use on the target (`tool::Wear`): a block broken,
    /// more if the tool is not made for it, or soil tilled. Skill saves a use now and then;
    /// worn out, it breaks to nothing (or a head a pack names).
    WearHeld,
    /// Coat the target in place with what is held (see `Requirement::Coats`).
    Coat,
    /// Turn the target into what it swaps for, its upper or lower half with it: open or
    /// shut a door (see [`swapping`]).
    Swap,
    /// Light the target again: a stub into its torch (`flame`), burning anew.
    Relight,
    /// Work at the target crafting table: its three-by-three grid, until the actor walks away.
    UseTable,
    /// Empty the pouch in hand into the actor's inventory.
    EmptyPouch,
    /// Bring up what bites in the target water, if anything (`fishing.ron`).
    Fish,
    /// Sleep at the target bed: the actor wakes there after dying, until they have a home.
    Sleep,
    /// Sit on the target seat: the actor is set on it, and sits until they move off.
    Sit,
    /// Light the multiblock whose core is the target, if it stands around it: the
    /// Homestead Hearth becomes the actor's home, a crucible kiln starts smelting.
    Light,
    /// Use the target machine: what the actor holds goes in if it takes it, otherwise
    /// what it made comes out; the actor is told how it is doing.
    UseMachine,
    /// Pick the lock of the target chest with what the actor holds (`treasure.ron`): open,
    /// it becomes a plain chest holding what it rolled.
    PickLock,
    /// Pray at the target altar: offer one held offering for Magic experience, or with none
    /// in hand have prayer points back in full (`prayer`).
    Pray,
    /// Take from the target's bees with what the actor holds (`bees.ron`): calmly under
    /// smoke, stung without it.
    TendBees,
    /// Use the target home station (`stations.ron`): load it with what is in hand, take
    /// what it made, or be told how it goes.
    UseStation,
    /// Take what the target sprung trap caught (`traps.ron`), and set it again.
    TakeFromTrap,
    /// Spread one held fertiliser on the target crop: the crops within its reach grow.
    Fertilise,
    /// Open the target store beside the actor's inventory, its slots theirs to move items
    /// in and out of until the screen closes (`inventory::STORE`).
    OpenStore,
    /// Put the actor's pack away into their home's stores: each stack off the hotbar whose
    /// item a store there already holds goes into that store, as far as it fits.
    PutAway,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionDef {
    pub name: String,
    pub duration: Duration,
    #[serde(default)]
    pub requires: Vec<Requirement>,
    pub effects: Vec<Effect>,
}

#[derive(Deserialize)]
struct ActionFile {
    actions: Vec<ActionDef>,
}

#[derive(Debug, Default)]
pub struct ActionRegistry {
    defs: Vec<ActionDef>,
    by_name: FxHashMap<String, ActionId>,
}

impl ActionRegistry {
    pub fn from_ron(namespace: &str, source: &str) -> Result<Self, RegistryError> {
        let mut registry = Self::default();
        registry.extend(namespace, source)?;
        Ok(registry)
    }

    /// Adds a package's actions. Unqualified names are placed in `namespace`.
    pub fn extend(&mut self, namespace: &str, source: &str) -> Result<(), RegistryError> {
        let file: ActionFile =
            ron::from_str(source).map_err(|e| RegistryError(format!("{namespace}/actions.ron: {e}")))?;
        for mut def in file.actions {
            if !def.name.contains(':') {
                def.name = format!("{namespace}:{}", def.name);
            }
            let id = ActionId(self.defs.len() as u16);
            if self.by_name.insert(def.name.clone(), id).is_some() {
                return Err(RegistryError(format!("duplicate action `{}`", def.name)));
            }
            self.defs.push(def);
        }
        Ok(())
    }

    pub fn get(&self, id: ActionId) -> Option<&ActionDef> {
        self.defs.get(id.0 as usize)
    }

    pub fn id(&self, name: &str) -> Option<ActionId> {
        self.by_name.get(name).copied()
    }

    /// Every action's full name (`core:eat`), in the order they were added.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.defs.iter().map(|def| def.name.as_str())
    }

    pub fn expect(&self, name: &str) -> ActionId {
        self.id(name).unwrap_or_else(|| panic!("content is missing required action `{name}`"))
    }

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

/// What validation needs to know about the world and the actor. Implemented by the
/// server (authoritative) and the client (prediction).
pub trait ActionContext {
    fn block(&self, pos: IVec3) -> Option<BlockId>;
    fn info(&self, block: BlockId) -> &BlockInfo;
    fn blocks(&self) -> &BlockRegistry;
    fn crops(&self) -> &CropRegistry;
    /// The actor's level in `skill`.
    fn actor_level(&self, skill: SkillId) -> u8;
    /// The actor's own level in `skill`, without boosts (what wields a weapon).
    fn actor_own_level(&self, skill: SkillId) -> u8 {
        self.actor_level(skill)
    }
    fn actor_eye(&self) -> Vec3;
    fn actor_alive(&self) -> bool;
    fn actor_held(&self) -> Option<BlockId>;
    /// Everything the actor carries in its slots, the hand's among them (a tool a recipe
    /// asks for, `Requirement::CookingTool`).
    fn actor_carried(&self) -> Vec<BlockId> {
        Vec::new()
    }
    /// What the actor's pouches carry (see [`crate::inventory::Inventory::pouched`]).
    fn actor_pouched(&self) -> Vec<BlockId> {
        Vec::new()
    }
    /// The seed the actor would sow: the one in hand, or with a seed pouch in hand, the
    /// first seed in it the actor's level allows.
    fn actor_seed(&self) -> Option<BlockId> {
        let crops = self.crops();
        let held = self.actor_held()?;
        if crops.planted_by(held).is_some() {
            return Some(held);
        }
        self.actor_pouched().into_iter().find(|&seed| {
            crate::inventory::pouch_for(seed) == Some(held)
                && crops.planted_by(seed).is_some_and(|crop| self.actor_level(crops.get(crop).skill) >= crops.get(crop).def.level)
        })
    }
    /// Whether the actor has health to heal; true unless a context knows otherwise.
    fn actor_hurt(&self) -> bool {
        true
    }
    /// How fast the actor mines, from its effects (1 normally).
    fn actor_mining_speed(&self) -> f32 {
        1.0
    }
    /// What tools do, where the context knows.
    fn tools(&self) -> Option<&crate::tool::ToolRegistry> {
        None
    }
    /// What burns out and is lit again, where the context knows.
    fn flames(&self) -> Option<&crate::flame::Flames> {
        None
    }
    /// Whether any agent's body overlaps the cell.
    fn occupied(&self, cell: IVec3) -> bool;
    /// Whether a creature's body overlaps the cell; false where the context knows none.
    fn creature_in(&self, _cell: IVec3) -> bool {
        false
    }
    /// The cooking recipes, where the context knows them.
    fn cooking(&self) -> Option<&crate::cooking::CookingRegistry> {
        None
    }
    /// The machines, where the context knows them.
    fn machines(&self) -> Option<&crate::machine::MachineRegistry> {
        None
    }
    /// The multiblocks, where the context knows them.
    fn multiblocks(&self) -> Option<&crate::multiblock::MultiblockRegistry> {
        None
    }
    /// What bees give, where the context knows.
    fn bees(&self) -> Option<&crate::bees::Bees> {
        None
    }
    /// The home stations and fertilisers, where the context knows.
    fn stations(&self) -> Option<&crate::station::Stations> {
        None
    }
    /// The traps, where the context knows them.
    fn traps(&self) -> Option<&crate::trap::Traps> {
        None
    }
    /// Whether the creature with that id takes `item` from the actor's hand.
    fn creature_accepts(&self, _id: u32, _item: BlockId) -> bool {
        false
    }
    /// Whether the creature with that id is one the actor keeps.
    fn creature_is_mine(&self, _id: u32) -> bool {
        false
    }
    /// The box of a living creature, if there is one with that id.
    fn creature_box(&self, _id: u32) -> Option<crate::physics::Aabb> {
        None
    }
    /// Weapons, where the context knows them.
    fn combat(&self) -> Option<&crate::combat::CombatRegistry> {
        None
    }
}

/// Whether creature `id` is alive and its box within `reach` of the actor's eye.
fn creature_within(ctx: &impl ActionContext, id: u32, reach: f32) -> bool {
    ctx.creature_box(id).is_some_and(|aabb| {
        let nearest = ctx.actor_eye().clamp(aabb.min, aabb.max);
        ctx.actor_eye().distance(nearest) <= reach
    })
}

/// Where a block placed against `face` of `block` goes: into `block` itself when it can
/// be replaced (aiming at grass puts the block on the ground beneath it), otherwise into
/// the cell in front of the face.
pub fn placement_cell(ctx: &impl ActionContext, block: IVec3, face: Face) -> IVec3 {
    if ctx.block(block).is_some_and(|b| ctx.info(b).replaceable) { block } else { block + face.normal() }
}

/// The block placing `held` against `face` of `block` puts there: against a side it does
/// not go into, what it goes on walls as (a wall torch, facing out); otherwise itself,
/// upside down against the underside of a block if it comes that way (a top slab), and
/// facing the actor if it comes in facings (stairs climb away from whoever placed them).
pub fn placed_block(ctx: &impl ActionContext, held: BlockId, block: IVec3, face: Face) -> BlockId {
    let blocks = ctx.blocks();
    let cell = placement_cell(ctx, block, face);
    let against = cell != block;
    if let (Some(wall), Some(facing)) = (ctx.info(held).wall, Facing::of_face(face))
        && against
    {
        return blocks.facing(wall, facing);
    }
    let mut placed = held;
    if against && face == Face::NegY {
        placed = blocks.flipped(held).unwrap_or(held);
    }
    if ctx.info(placed).facing.is_some() && ctx.info(placed).support != Support::Wall {
        let toward = ctx.actor_eye() - (cell.as_vec3() + Vec3::splat(0.5));
        let facing = if toward.x.abs() > toward.z.abs() {
            if toward.x > 0.0 { Facing::East } else { Facing::West }
        } else if toward.z > 0.0 {
            Facing::South
        } else {
            Facing::North
        };
        placed = blocks.facing(placed, facing);
    }
    placed
}

/// Whether the actor may harvest what is at `cell`: a ripe crop (or fruit) asks for its
/// level; anything else is fine.
pub fn crop_level_met(ctx: &impl ActionContext, cell: IVec3) -> bool {
    let crops = ctx.crops();
    ctx.block(cell).and_then(|b| crops.stage_of(b)).is_none_or(|(crop, stage)| {
        let crop = crops.get(crop);
        stage != crop.ripe_stage() || ctx.actor_level(crop.skill) >= crop.def.level
    })
}

/// The tag of a block a player can sit on (`Requirement::Seat`).
pub const SEAT_TAG: &str = "seat";

/// What using the block at `pos` changes (see [`crate::block::BlockDef::swaps`]): it and
/// its other half (the upper half above a lower one, or the lower below an upper), each
/// with what it turns into. Empty when it swaps for nothing.
pub fn swapping(ctx: &impl ActionContext, pos: IVec3) -> Vec<(IVec3, BlockId)> {
    let blocks = ctx.blocks();
    let Some(here) = ctx.block(pos) else { return Vec::new() };
    let Some(to) = blocks.swapped(here) else { return Vec::new() };
    let mut changes = vec![(pos, to)];
    let half = |lower: BlockId, upper: BlockId| blocks.upper(lower) == Some(upper);
    if let Some(above) = ctx.block(pos + IVec3::Y).filter(|&up| half(here, up))
        && let Some(to) = blocks.swapped(above)
    {
        changes.push((pos + IVec3::Y, to));
    } else if let Some(below) = ctx.block(pos - IVec3::Y).filter(|&low| half(low, here))
        && let Some(to) = blocks.swapped(below)
    {
        changes.push((pos - IVec3::Y, to));
    }
    if let Some(other) = pair(ctx, pos)
        && let Some(to) = ctx.block(other).and_then(|b| blocks.swapped(b))
    {
        changes.push((other, to));
    }
    changes
}

/// The cell of the other of a pair set side by side (see [`crate::block::BlockDef::beside`])
/// with the block at `pos`: the block beside it, or the one it is beside.
pub fn pair(ctx: &impl ActionContext, pos: IVec3) -> Option<IVec3> {
    let blocks = ctx.blocks();
    let here = ctx.block(pos)?;
    if let Some((by, partner)) = blocks.beside(here) {
        return (ctx.block(pos + by) == Some(partner)).then_some(pos + by);
    }
    Facing::ALL.into_iter().map(|f| pos - f.beside()).find(|&at| {
        ctx.block(at).and_then(|b| blocks.beside(b)).is_some_and(|(by, partner)| at + by == pos && partner == here)
    })
}

/// The blocks resting on `pos` that go when it is cleared: the plants standing on it (a
/// flower, both halves of tall grass, a stalk of sugar cane), the column hanging from it
/// (vines, roots, a stalactite), what is fixed to its sides (wall torches), and the lower
/// half of a block whose upper half it is (a door), and the other of a pair set side by side
/// (a double gate's leaves).
pub fn supported_by(ctx: &impl ActionContext, pos: IVec3) -> Vec<IVec3> {
    let column = |step: i32, support: Support| {
        (1..).map(move |d| pos + IVec3::Y * (d * step)).take_while(move |&p| ctx.block(p).is_some_and(|b| ctx.info(b).support == support))
    };
    let walls = Facing::ALL.into_iter().map(move |f| pos + f.normal()).filter(move |&p| {
        ctx.block(p).is_some_and(|b| ctx.info(b).support == Support::Wall && ctx.info(b).facing.is_some_and(|f| p - f.normal() == pos))
    });
    let lower = ctx.block(pos).zip(ctx.block(pos - IVec3::Y)).filter(|&(here, below)| ctx.blocks().upper(below) == Some(here)).map(|_| pos - IVec3::Y);
    column(1, Support::Below).chain(column(-1, Support::Above)).chain(walls).chain(lower).chain(pair(ctx, pos)).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    WrongTarget,
    Unmet(Requirement),
}

impl ActionDef {
    /// Checks every requirement against the current state.
    pub fn validate(&self, ctx: &impl ActionContext, target: ActionTarget) -> Result<(), Rejection> {
        for &requirement in &self.requires {
            if !self.holds(requirement, ctx, target)? {
                return Err(Rejection::Unmet(requirement));
            }
        }
        Ok(())
    }

    fn holds(&self, requirement: Requirement, ctx: &impl ActionContext, target: ActionTarget) -> Result<bool, Rejection> {
        let block_target = || match target {
            ActionTarget::Block { block, face } => Ok((block, face)),
            _ => Err(Rejection::WrongTarget),
        };
        Ok(match requirement {
            Requirement::Alive => ctx.actor_alive(),
            Requirement::Dead => !ctx.actor_alive(),
            Requirement::InReach => {
                let (block, _) = block_target()?;
                ctx.actor_eye().distance(block.as_vec3() + 0.5) <= REACH + 1.5
            }
            Requirement::Breakable => {
                let (block, _) = block_target()?;
                ctx.block(block).is_some_and(|b| !b.is_air() && ctx.info(b).hardness >= 0.0)
            }
            Requirement::HoldingBlock => ctx.actor_held().is_some_and(|b| ctx.info(b).placeable),
            Requirement::HoldingSeed => ctx.actor_seed().is_some(),
            Requirement::Plantable => {
                let (block, face) = block_target()?;
                let Some(crop) = ctx.actor_seed().and_then(|b| ctx.crops().planted_by(b)) else { return Ok(false) };
                let ground = ctx.block(block).map(|b| ctx.blocks().def(b));
                let fits = ground.is_some_and(|def| ctx.crops().get(crop).def.ground.iter().any(|tag| def.has_tag(tag)));
                let room = ctx.block(block + IVec3::Y).is_some_and(|b| b.is_air() || (ctx.info(b).replaceable && ctx.info(b).fluid.is_none()));
                face == Face::PosY && fits && room
            }
            Requirement::HoldingFood => ctx.actor_held().is_some_and(|b| ctx.info(b).food > 0),
            Requirement::Hurt => ctx.actor_hurt(),
            Requirement::SkillLevel => match ctx.actor_seed().and_then(|b| ctx.crops().planted_by(b)) {
                Some(crop) => {
                    let crop = ctx.crops().get(crop);
                    ctx.actor_level(crop.skill) >= crop.def.level
                }
                None => true,
            },
            Requirement::Ripe => {
                let (block, _) = block_target()?;
                let crops = ctx.crops();
                ctx.block(block)
                    .and_then(|b| crops.stage_of(b))
                    .is_some_and(|(crop, stage)| stage == crops.get(crop).ripe_stage() && crops.get(crop).def.regrow.is_some())
            }
            Requirement::CreatureAccepts => {
                let ActionTarget::Creature(id) = target else { return Err(Rejection::WrongTarget) };
                ctx.actor_held().is_some_and(|item| ctx.creature_accepts(id, item))
            }
            Requirement::CreatureIsMine => {
                let ActionTarget::Creature(id) = target else { return Err(Rejection::WrongTarget) };
                ctx.creature_is_mine(id)
            }
            Requirement::Cookable | Requirement::CookingLevel => {
                let (block, _) = block_target()?;
                let station = ctx.block(block).map(|b| ctx.blocks().def(b).tags.as_slice()).unwrap_or(&[]);
                let recipe = ctx.actor_held().zip(ctx.cooking()).and_then(|(item, cooking)| cooking.for_item(item, station));
                match (requirement, recipe) {
                    (Requirement::Cookable, recipe) => recipe.is_some(),
                    (_, Some(recipe)) => recipe.skill.is_none_or(|skill| ctx.actor_level(skill) >= recipe.def.level),
                    (_, None) => true,
                }
            }
            Requirement::CookingTool => {
                let (block, _) = block_target()?;
                let station = ctx.block(block).map(|b| ctx.blocks().def(b).tags.as_slice()).unwrap_or(&[]);
                let recipe = ctx.actor_held().zip(ctx.cooking()).and_then(|(item, cooking)| cooking.for_item(item, station));
                recipe.is_none_or(|recipe| recipe.has_tool(ctx.blocks(), ctx.actor_carried()))
            }
            Requirement::ToolGood => {
                let (block, _) = block_target()?;
                match (ctx.block(block), ctx.tools()) {
                    (Some(b), Some(tools)) => tools.good_enough(ctx.actor_held(), b),
                    _ => true,
                }
            }
            Requirement::CropLevel => {
                let (block, _) = block_target()?;
                // The target, and what stands on it: breaking the ground under a ripe crop
                // harvests it too.
                std::iter::once(block).chain(supported_by(ctx, block)).all(|cell| crop_level_met(ctx, cell))
            }
            Requirement::BlockLevel => {
                let (block, _) = block_target()?;
                let rule = ctx.block(block).zip(ctx.tools()).and_then(|(b, tools)| tools.rule(b));
                rule.is_none_or(|rule| rule.skill.is_none_or(|skill| ctx.actor_level(skill) >= rule.level))
            }
            Requirement::Relights => {
                let (block, _) = block_target()?;
                ctx.flames().is_some_and(|flames| ctx.block(block).zip(ctx.actor_held()).is_some_and(|(b, item)| flames.lights(ctx.blocks(), b, item)))
            }
            Requirement::Swaps => {
                let (block, _) = block_target()?;
                let changes = swapping(ctx, block);
                !changes.is_empty() && changes.iter().all(|&(cell, to)| !ctx.info(to).solid || !(ctx.occupied(cell) || ctx.creature_in(cell)))
            }
            Requirement::Tillable => {
                let (block, _) = block_target()?;
                let tilled = ctx.block(block).map(|b| ctx.blocks().def(b));
                let soil = tilled.is_some_and(|def| def.has_tag("soil") && !def.has_tag("tilled"));
                soil && ctx.block(block + IVec3::Y).is_some_and(|b| b.is_air() || (ctx.info(b).replaceable && ctx.info(b).fluid.is_none()))
            }
            Requirement::Fishable => {
                let (block, _) = block_target()?;
                let rod = ctx.actor_held().is_some_and(|b| ctx.blocks().def(b).has_tag("rod"));
                // Any fluid but a hot one (lava): water, its flows and rivers.
                let water = |p: IVec3| ctx.block(p).is_some_and(|b| ctx.info(b).fluid.is_some() && !ctx.blocks().def(b).has_tag("hot"));
                let open = ctx.block(block + IVec3::Y).is_some_and(|b| b.is_air() || water(block + IVec3::Y));
                rod && water(block) && open && ctx.actor_eye().distance(block.as_vec3() + 0.5) <= CAST
            }
            Requirement::CraftingTable => {
                let (block, _) = block_target()?;
                ctx.block(block).is_some_and(|b| ctx.blocks().def(b).has_tag("crafting_table"))
            }
            Requirement::Bed => {
                let (block, _) = block_target()?;
                ctx.block(block).is_some_and(|b| ctx.blocks().def(b).has_tag("bed"))
            }
            Requirement::Coats => {
                // On a wall's face only: clay to the top of a dirt floor or a panel is still
                // laid there as a block.
                let (block, face) = block_target()?;
                let side = !matches!(face, Face::PosY | Face::NegY);
                let coating = ctx.block(block).and_then(|b| ctx.blocks().coating(b));
                side && coating.is_some_and(|(with, _)| ctx.actor_held() == Some(with))
            }
            Requirement::CoatClear => {
                let (block, _) = block_target()?;
                let into = ctx.block(block).and_then(|b| ctx.blocks().coating(b)).map(|(_, into)| into);
                into.is_none_or(|into| !ctx.info(into).solid || !(ctx.occupied(block) || ctx.creature_in(block)))
            }
            Requirement::Seat => {
                let (block, _) = block_target()?;
                let seat = ctx.block(block).is_some_and(|b| ctx.blocks().def(b).has_tag(SEAT_TAG));
                let free = |p: IVec3| ctx.block(p).is_some_and(|b| !ctx.info(b).solid);
                let feet = crate::vitals::seated_feet(block, ctx.block(block).is_some_and(|b| ctx.info(b).solid)).floor().as_ivec3();
                let hand_free = ctx.actor_held().is_none_or(|item| !ctx.info(item).placeable);
                seat && free(feet) && free(feet + IVec3::Y) && hand_free
            }
            Requirement::Locked => {
                let (block, _) = block_target()?;
                ctx.block(block).is_some_and(|b| ctx.blocks().def(b).has_tag(crate::treasure::LOCKED_TAG))
            }
            Requirement::Bees => {
                let (block, _) = block_target()?;
                let Some(bees) = ctx.bees() else { return Ok(false) };
                ctx.block(block).is_some_and(|b| bees.take(b, ctx.actor_held()).is_some())
            }
            Requirement::Station => {
                let (block, _) = block_target()?;
                let Some(stations) = ctx.stations() else { return Ok(false) };
                ctx.block(block).is_some_and(|b| stations.usable(b, ctx.actor_held()))
            }
            Requirement::SprungTrap => {
                let (block, _) = block_target()?;
                let Some(traps) = ctx.traps() else { return Ok(false) };
                ctx.block(block).is_some_and(|b| traps.sprung(b).is_some())
            }
            Requirement::Fertilisable => {
                let (block, _) = block_target()?;
                let Some(stations) = ctx.stations() else { return Ok(false) };
                let crops = ctx.crops();
                let growing = ctx.block(block).and_then(|b| crops.stage_of(b)).is_some_and(|(crop, stage)| stage < crops.get(crop).ripe_stage());
                growing && ctx.actor_held().is_some_and(|item| stations.fertiliser(item).is_some())
            }
            Requirement::MultiblockCore => {
                let (block, _) = block_target()?;
                // Any cell of a standing clamp, with what lights it in hand, is as its core:
                // the firelighter lights it, never laid on its earth.
                let smoulders = || {
                    let (_, structure) = ctx.multiblocks()?.smoulder_round(block, |p| ctx.block(p))?;
                    let lit_by = structure.lit_by.as_deref()?;
                    Some(ctx.actor_held().is_some_and(|item| ctx.blocks().def(item).has_tag(lit_by)))
                };
                let Some(core) = ctx.block(block).filter(|&b| ctx.blocks().def(b).has_tag("multiblock_core")) else { return Ok(smoulders() == Some(true)) };
                // A machine's core with a block in hand that it does not take (use_machine
                // takes what it does) has that block built against it, as any machine does.
                let machine = ctx.machines().is_some_and(|machines| machines.get(core).is_some());
                !(machine && ctx.actor_held().is_some_and(|item| ctx.info(item).placeable))
            }
            Requirement::Machine => {
                let (block, _) = block_target()?;
                let Some(machines) = ctx.machines() else { return Ok(false) };
                let Some(machine) = ctx.block(block).and_then(|b| machines.get(b)) else { return Ok(false) };
                match ctx.actor_held() {
                    None => true,
                    Some(item) => {
                        let fuel = machine.burns() && machines.burns_in(machine, item).is_some();
                        let input = machine.is_processor() && machines.takes(machine, item);
                        // A bin takes anything but machines: a chute in hand is laid against it.
                        let bin = machine.bin_takes(item) && !ctx.blocks().def(item).has_tag(&machines.rules.machine_tag);
                        fuel || input || bin || !ctx.info(item).placeable
                    }
                }
            }
            Requirement::Store => {
                let (block, _) = block_target()?;
                let Some(machines) = ctx.machines() else { return Ok(false) };
                let store = ctx.block(block).and_then(|b| machines.get(b)).is_some_and(|m| m.is_store());
                store && !ctx.actor_held().is_some_and(|item| ctx.blocks().def(item).has_tag(&machines.rules.machine_tag))
            }
            Requirement::Altar => {
                let (block, _) = block_target()?;
                ctx.block(block).is_some_and(|b| ctx.blocks().def(b).has_tag("altar"))
            }
            Requirement::CreatureInReach => {
                let ActionTarget::Creature(id) = target else { return Err(Rejection::WrongTarget) };
                // What is held reaches (a spear further, a dagger less, for one who can wield
                // it), with slack for the player's view of the creature running behind.
                let feel = crate::combat::Feel::default();
                let reach = ctx.combat().map_or(feel.reach + feel.reach_slack, |combat| {
                    combat.wielded(ctx.actor_held(), |skill| ctx.actor_own_level(skill)).reach + combat.feel.reach_slack
                });
                creature_within(ctx, id, reach)
            }
            Requirement::CreatureInTouch => {
                let ActionTarget::Creature(id) = target else { return Err(Rejection::WrongTarget) };
                let feel = ctx.combat().map_or_else(crate::combat::Feel::default, |combat| combat.feel);
                creature_within(ctx, id, feel.touch + feel.reach_slack)
            }
            Requirement::Replaceable => {
                let (block, face) = block_target()?;
                let cell = placement_cell(ctx, block, face);
                // A block placed with others (an upper half, a second leaf beside it) needs
                // their cells free too.
                let free = |at: IVec3| ctx.block(at).is_some_and(|b| ctx.info(b).replaceable);
                let placed = ctx.actor_held().map(|held| placed_block(ctx, held, block, face));
                free(cell) && placed.is_none_or(|placed| ctx.blocks().placed_with(placed).all(|(by, _)| free(cell + by)))
            }
            Requirement::Unobstructed => {
                let (block, face) = block_target()?;
                let solid = ctx.actor_held().is_some_and(|b| ctx.info(b).solid);
                let cell = placement_cell(ctx, block, face);
                let placed = ctx.actor_held().map(|held| placed_block(ctx, held, block, face));
                // A player or a creature standing there keeps a solid block out, and out of
                // every cell a block placed with it fills (a door's top, a double gate's leaf).
                let taken = |p: IVec3| ctx.occupied(p) || ctx.creature_in(p);
                let with_clear = placed.is_none_or(|placed| ctx.blocks().placed_with(placed).all(|(by, b)| !ctx.info(b).solid || !taken(cell + by)));
                (!solid || !taken(cell)) && with_clear
            }
            Requirement::Supported => {
                let (block, face) = block_target()?;
                let cell = placement_cell(ctx, block, face);
                let solid = |p: IVec3| ctx.block(p).is_some_and(|b| ctx.info(b).solid);
                let Some(placed) = ctx.actor_held().map(|held| placed_block(ctx, held, block, face)) else { return Ok(true) };
                match ctx.info(placed).support {
                    Support::None => true,
                    Support::Below => solid(cell - IVec3::Y),
                    Support::Above => solid(cell + IVec3::Y),
                    Support::Wall => ctx.info(placed).facing.is_some_and(|f| solid(cell - f.normal())),
                }
            }
        })
    }

    /// Seconds until the action completes.
    pub fn seconds(&self, ctx: &impl ActionContext, target: ActionTarget) -> f32 {
        match self.duration {
            Duration::Instant => 0.0,
            Duration::Seconds(s) => s,
            Duration::TargetHardness => match target {
                ActionTarget::Block { block, .. } => ctx.block(block).map_or(0.0, |b| {
                    let tool = ctx.tools().map_or(1.0, |tools| tools.speed(ctx.actor_held(), b));
                    ctx.info(b).hardness.max(0.0) / (ctx.actor_mining_speed() * tool).max(0.05)
                }),
                _ => 0.0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use rustc_hash::FxHashMap;

    use super::*;
    use crate::block::{BlockDef, BlockRegistry, RenderKind};

    struct World {
        registry: BlockRegistry,
        blocks: FxHashMap<IVec3, BlockId>,
        held: BlockId,
        crops: CropRegistry,
    }

    impl ActionContext for World {
        fn block(&self, pos: IVec3) -> Option<BlockId> {
            Some(self.blocks.get(&pos).copied().unwrap_or(BlockId::AIR))
        }
        fn info(&self, block: BlockId) -> &BlockInfo {
            self.registry.info(block)
        }
        fn blocks(&self) -> &BlockRegistry {
            &self.registry
        }
        fn crops(&self) -> &CropRegistry {
            &self.crops
        }
        fn actor_level(&self, _: SkillId) -> u8 {
            1
        }
        fn actor_eye(&self) -> Vec3 {
            Vec3::new(0.5, 2.6, 2.5)
        }
        fn actor_alive(&self) -> bool {
            true
        }
        fn actor_held(&self) -> Option<BlockId> {
            Some(self.held)
        }
        fn occupied(&self, _: IVec3) -> bool {
            false
        }
    }

    const STONE: BlockId = BlockId(1);
    const FLOWER: BlockId = BlockId(2);
    const VINE: BlockId = BlockId(3);

    /// Stone at the origin with a flower on it, and bare stone next to it along +z.
    fn world(held: BlockId) -> World {
        let registry = BlockRegistry::new(vec![
            BlockDef { name: "test:stone".into(), ..Default::default() },
            BlockDef { name: "test:flower".into(), render: RenderKind::Plant, solid: false, hardness: 0.0, ..Default::default() },
            BlockDef {
                name: "test:vine".into(),
                render: RenderKind::Plant,
                solid: false,
                hardness: 0.0,
                support: Some(Support::Above),
                ..Default::default()
            },
            BlockDef {
                name: "test:torch".into(),
                render: RenderKind::Plant,
                solid: false,
                wall: Some("test:wall_torch".into()),
                ..Default::default()
            },
            BlockDef {
                name: "test:wall_torch".into(),
                render: RenderKind::Plant,
                solid: false,
                drops: Some("test:torch".into()),
                facing: true,
                support: Some(Support::Wall),
                ..Default::default()
            },
        ])
        .unwrap();
        let blocks = [(IVec3::ZERO, STONE), (IVec3::Y, FLOWER), (IVec3::Z, STONE)].into_iter().collect();
        World { registry, blocks, held, crops: CropRegistry::default() }
    }

    const TORCH: BlockId = BlockId(4);

    #[test]
    fn stairs_face_whoever_places_them_and_go_upside_down_under_a_ceiling() {
        let file = crate::shape::ShapeFile::read("test", r#"(shapes: {"stairs": (boxes: [(to: (16, 8, 16)), (from: (0, 8, 8))])})"#);
        let limits = crate::shape::ShapeLimits { boxes: 2, quads: 64 };
        let shapes = crate::shape::Shapes::gather(vec![("test".into(), file.unwrap())], limits).unwrap();
        let stairs = BlockDef { name: "test:stairs".into(), render: RenderKind::Model, shape: Some("test:stairs".into()), facing: true, flips: true, ..Default::default() };
        let registry = BlockRegistry::with_shapes(vec![BlockDef { name: "test:stone".into(), ..Default::default() }, stairs], &shapes).unwrap();
        let held = registry.expect("test:stairs");
        let blocks = [(IVec3::ZERO, STONE)].into_iter().collect();
        let world = World { registry, blocks, held, crops: CropRegistry::default() };
        let name = |block: BlockId| world.registry.def(block).name.clone();
        // On top of the stone, with the actor's eye south of it (+z): they climb northwards,
        // away from the actor.
        assert_eq!(name(placed_block(&world, held, IVec3::ZERO, Face::PosY)), "test:stairs_south");
        // Against the stone's west side, the actor still to the south.
        assert_eq!(name(placed_block(&world, held, IVec3::ZERO, Face::NegX)), "test:stairs_south");
        // Against its underside they hang upside down.
        assert_eq!(name(placed_block(&world, held, IVec3::ZERO, Face::NegY)), "test:stairs_top_south");
    }

    #[test]
    fn a_torch_against_a_wall_is_a_wall_torch_facing_out_and_goes_with_the_wall() {
        let mut world = world(TORCH);
        let (registry, east) = (&world.registry, Face::PosX);
        let placed = placed_block(&world, TORCH, IVec3::ZERO, east);
        assert_eq!(registry.def(placed).name, "test:wall_torch_east");
        assert_eq!(registry.info(placed).facing, Some(Facing::East));
        assert_eq!(registry.info(placed).drops, TORCH);
        // On top of a block it is the torch itself.
        assert_eq!(placed_block(&world, TORCH, IVec3::ZERO, Face::PosY), TORCH);
        assert!(place().validate(&world, ActionTarget::Block { block: IVec3::ZERO, face: east }).is_ok());
        world.blocks.insert(IVec3::X, placed);
        assert!(supported_by(&world, IVec3::ZERO).contains(&IVec3::X));
        // The stone beside it along +z holds nothing.
        assert!(!supported_by(&world, IVec3::Z).contains(&IVec3::X));
        assert_eq!(world.registry.facing(placed, Facing::West), world.registry.id("test:wall_torch_west").unwrap());
        assert_eq!(world.registry.facing(placed, Facing::North), world.registry.id("test:wall_torch").unwrap());
    }

    #[test]
    fn facings_turn_clockwise_from_north() {
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-6);
        assert!(near(Facing::East.turn([0.5, 0.8, 0.75]), [0.25, 0.8, 0.5]));
        assert!(near(Facing::South.turn([0.5, 0.8, 0.75]), [0.5, 0.8, 0.25]));
        assert!(near(Facing::West.turn([0.5, 0.8, 0.75]), [0.75, 0.8, 0.5]));
        for facing in Facing::ALL {
            // The wall a torch facing north rests on (+z) turns to the one behind it.
            let wall = facing.turn([0.5, 0.5, 1.0]);
            assert_eq!(IVec3::new((wall[0] * 2.0 - 1.0).round() as i32, 0, (wall[2] * 2.0 - 1.0).round() as i32), -facing.normal());
        }
    }

    fn place() -> ActionDef {
        use Requirement::*;
        ActionDef {
            name: "test:place".into(),
            duration: Duration::Instant,
            requires: vec![Alive, InReach, HoldingBlock, Replaceable, Unobstructed, Supported],
            effects: vec![Effect::PlaceHeld],
        }
    }

    /// A click on a machine's core (a bloomery) with a block in hand it does not take lays
    /// the block against it, as on any machine, rather than lighting it; an empty hand or
    /// A creature is touched (tended, looked at closely, let go) from `Feel::touch` away
    /// whatever is in hand, but struck only within the held weapon's reach.
    #[test]
    fn a_creature_is_touched_further_than_a_fist_strikes() {
        struct Near<'a> {
            content: &'a crate::content::Content,
            held: Option<BlockId>,
            /// Blocks from the actor's eye to the creature's nearest face.
            away: f32,
        }
        impl ActionContext for Near<'_> {
            fn block(&self, _: IVec3) -> Option<BlockId> {
                Some(BlockId::AIR)
            }
            fn info(&self, block: BlockId) -> &BlockInfo {
                self.content.blocks.info(block)
            }
            fn blocks(&self) -> &BlockRegistry {
                &self.content.blocks
            }
            fn crops(&self) -> &CropRegistry {
                &self.content.crops
            }
            fn actor_level(&self, _: SkillId) -> u8 {
                1
            }
            fn actor_eye(&self) -> Vec3 {
                Vec3::ZERO
            }
            fn actor_alive(&self) -> bool {
                true
            }
            fn actor_held(&self) -> Option<BlockId> {
                self.held
            }
            fn occupied(&self, _: IVec3) -> bool {
                false
            }
            fn creature_box(&self, _: u32) -> Option<crate::physics::Aabb> {
                Some(crate::physics::Aabb { min: Vec3::new(self.away, -1.0, -0.5), max: Vec3::new(self.away + 1.0, 1.0, 0.5) })
            }
            fn combat(&self) -> Option<&crate::combat::CombatRegistry> {
                Some(&self.content.combat)
            }
        }
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let action = |name: &str| content.actions.get(content.actions.id(name).unwrap()).unwrap();
        let (inspect, attack) = (action("base:inspect"), action("core:attack"));
        let feel = content.combat.feel;
        let fist = content.combat.wielded(None, |_| 1).reach;
        assert!(feel.touch > fist, "touch {} is no further than a fist's reach {fist}", feel.touch);
        let at = |away: f32| Near { content: &content, held: None, away };
        let target = ActionTarget::Creature(1);
        // Within the fist's reach (and its slack): struck and touched.
        assert_eq!(attack.validate(&at(fist + feel.reach_slack - 0.1), target), Ok(()));
        assert_eq!(inspect.validate(&at(fist + feel.reach_slack - 0.1), target), Ok(()));
        // Past it, within touch: touched, not struck.
        let between = (fist + feel.touch) / 2.0 + feel.reach_slack;
        assert_eq!(attack.validate(&at(between), target), Err(Rejection::Unmet(Requirement::CreatureInReach)));
        assert_eq!(inspect.validate(&at(between), target), Ok(()));
        // Past touch and its slack: neither.
        assert_eq!(inspect.validate(&at(feel.touch + feel.reach_slack + 0.1), target), Err(Rejection::Unmet(Requirement::CreatureInTouch)));
    }

    /// a tool lights it, and a hearth's core (no machine) lights whatever is in hand.
    #[test]
    fn a_block_in_hand_is_laid_against_a_machine_core_not_lit() {
        struct Core<'a> {
            content: &'a crate::content::Content,
            core: BlockId,
            held: Option<BlockId>,
        }
        impl ActionContext for Core<'_> {
            fn block(&self, pos: IVec3) -> Option<BlockId> {
                Some(if pos == IVec3::ZERO { self.core } else { BlockId::AIR })
            }
            fn info(&self, block: BlockId) -> &BlockInfo {
                self.content.blocks.info(block)
            }
            fn blocks(&self) -> &BlockRegistry {
                &self.content.blocks
            }
            fn crops(&self) -> &CropRegistry {
                &self.content.crops
            }
            fn actor_level(&self, _: SkillId) -> u8 {
                1
            }
            fn actor_eye(&self) -> Vec3 {
                Vec3::new(0.5, 2.6, 2.5)
            }
            fn actor_alive(&self) -> bool {
                true
            }
            fn actor_held(&self) -> Option<BlockId> {
                self.held
            }
            fn occupied(&self, _: IVec3) -> bool {
                false
            }
            fn machines(&self) -> Option<&crate::machine::MachineRegistry> {
                Some(&self.content.machines)
            }
        }
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let id = |name: &str| content.blocks.expect(name);
        let light = content.actions.get(content.actions.id("base:light").unwrap()).unwrap();
        let lights = |core: &str, held: Option<&str>| light.validate(&Core { content: &content, core: id(core), held: held.map(id) }, ActionTarget::Block { block: IVec3::ZERO, face: Face::PosZ });
        assert_eq!(lights("base:bloomery", Some("base:kiln_bricks")), Err(Rejection::Unmet(Requirement::MultiblockCore)));
        assert_eq!(lights("base:bloomery", None), Ok(()));
        assert_eq!(lights("base:bloomery", Some("base:iron_pickaxe")), Ok(()));
        assert_eq!(lights("base:crucible", Some("base:kiln_bricks")), Ok(()));
    }

    /// A clamp's earth, clicked from the floor beside it with what lights it in hand, is
    /// lit as its flue is, so the firelighter is never laid on it; with earth or nothing in
    /// hand it is not (earth is built on it), nor once a piece of it is gone.
    #[test]
    fn a_firelighter_lights_a_clamp_from_its_earth() {
        struct Clamp<'a> {
            content: &'a crate::content::Content,
            cells: std::collections::HashMap<IVec3, BlockId>,
            held: Option<BlockId>,
        }
        impl ActionContext for Clamp<'_> {
            fn block(&self, pos: IVec3) -> Option<BlockId> {
                Some(self.cells.get(&pos).copied().unwrap_or(BlockId::AIR))
            }
            fn info(&self, block: BlockId) -> &BlockInfo {
                self.content.blocks.info(block)
            }
            fn blocks(&self) -> &BlockRegistry {
                &self.content.blocks
            }
            fn crops(&self) -> &CropRegistry {
                &self.content.crops
            }
            fn actor_level(&self, _: SkillId) -> u8 {
                1
            }
            fn actor_eye(&self) -> Vec3 {
                Vec3::new(0.5, 1.62, -3.5)
            }
            fn actor_alive(&self) -> bool {
                true
            }
            fn actor_held(&self) -> Option<BlockId> {
                self.held
            }
            fn occupied(&self, _: IVec3) -> bool {
                false
            }
            fn multiblocks(&self) -> Option<&crate::multiblock::MultiblockRegistry> {
                Some(self.content.multiblocks.as_ref())
            }
        }
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let clamp = content.multiblocks.iter().find(|m| m.smoulder.is_some()).expect("a clamp");
        let cells: std::collections::HashMap<IVec3, BlockId> = clamp.example().into_iter().collect();
        let light = content.actions.get(content.actions.id("base:light").unwrap()).unwrap();
        let side = ActionTarget::Block { block: IVec3::NEG_Z, face: Face::NegZ };
        let lights = |cells: &std::collections::HashMap<IVec3, BlockId>, held: Option<&str>| light.validate(&Clamp { content: &content, cells: cells.clone(), held: held.map(|n| content.blocks.expect(n)) }, side);
        for firelighter in content.blocks.tagged(clamp.lit_by.as_deref().expect("lit by something")) {
            let name = content.blocks.def(firelighter).name.as_str();
            assert_eq!(lights(&cells, Some(name)), Ok(()), "lit from its earth with {name}");
        }
        let earth = cells[&IVec3::NEG_Z];
        assert_eq!(lights(&cells, Some(content.blocks.def(earth).name.as_str())), Err(Rejection::Unmet(Requirement::MultiblockCore)));
        assert_eq!(lights(&cells, None), Err(Rejection::Unmet(Requirement::MultiblockCore)));
        let mut opened = cells.clone();
        opened.insert(IVec3::Z, BlockId::AIR);
        assert_eq!(lights(&opened, Some("base:torch")), Err(Rejection::Unmet(Requirement::MultiblockCore)), "not standing");
    }

    #[test]
    fn blocks_replace_plants_they_are_aimed_at() {
        let world = world(STONE);
        // Aiming at the flower puts the block where the flower is, not on top of it.
        assert_eq!(placement_cell(&world, IVec3::Y, Face::PosY), IVec3::Y);
        assert!(place().validate(&world, ActionTarget::Block { block: IVec3::Y, face: Face::PosY }).is_ok());
        // Against the top of the stone beneath, it also goes into the flower's cell.
        let on_ground = ActionTarget::Block { block: IVec3::ZERO, face: Face::PosY };
        assert_eq!(placement_cell(&world, IVec3::ZERO, Face::PosY), IVec3::Y);
        assert!(place().validate(&world, on_ground).is_ok());
        assert_eq!(supported_by(&world, IVec3::ZERO), vec![IVec3::Y]);
        assert!(supported_by(&world, IVec3::Z).is_empty());
    }

    #[test]
    fn plants_need_solid_ground() {
        let world = world(FLOWER);
        let on_stone = ActionTarget::Block { block: IVec3::Z, face: Face::PosY };
        let in_the_air = ActionTarget::Block { block: IVec3::Z, face: Face::PosZ };
        assert!(place().validate(&world, on_stone).is_ok());
        assert_eq!(place().validate(&world, in_the_air), Err(Rejection::Unmet(Requirement::Supported)));
    }

    #[test]
    fn hanging_plants_need_a_ceiling_and_fall_with_it() {
        let mut world = world(VINE);
        // Under the stone at +z, not on top of it.
        let under_stone = ActionTarget::Block { block: IVec3::Z, face: Face::NegY };
        let on_stone = ActionTarget::Block { block: IVec3::Z, face: Face::PosY };
        assert!(place().validate(&world, under_stone).is_ok());
        assert_eq!(place().validate(&world, on_stone), Err(Rejection::Unmet(Requirement::Supported)));
        // A column of vines hangs under the stone; the flower on top stands on it.
        world.blocks.insert(IVec3::Z + IVec3::Y, FLOWER);
        for dy in 1..=3 {
            world.blocks.insert(IVec3::Z - IVec3::Y * dy, VINE);
        }
        world.blocks.insert(IVec3::Z - IVec3::Y * 5, VINE);
        // Clearing the stone takes the flower above and the three vines below, but not the
        // vine past the gap, which hangs from something else.
        let gone = supported_by(&world, IVec3::Z);
        assert_eq!(gone, vec![IVec3::new(0, 1, 1), IVec3::new(0, -1, 1), IVec3::new(0, -2, 1), IVec3::new(0, -3, 1)]);
        // Clearing a vine takes only the ones hanging below it.
        assert_eq!(supported_by(&world, IVec3::new(0, -2, 1)), vec![IVec3::new(0, -3, 1)]);
        // Plants default to standing on the ground; other blocks need nothing.
        assert_eq!(world.info(FLOWER).support, Support::Below);
        assert_eq!(world.info(STONE).support, Support::None);
    }
}
