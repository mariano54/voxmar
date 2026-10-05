//! Actions: the only way the world changes in discrete steps.
//!
//! Every action goes through one lifecycle, whoever performs it (player, machine, NPC):
//! propose, validate, schedule one completion, then commit atomically or cancel. Action
//! definitions are data; what they may require and do is a small, fixed vocabulary of
//! requirements and effects owned by the engine, which is what lets any package add
//! actions while the engine keeps every change validated, deterministic and networked.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::{BlockId, BlockInfo, BlockRegistry, RegistryError, Support};
use crate::coords::Face;
use crate::crop::CropRegistry;
use crate::skill::SkillId;
use crate::glam::{IVec3, Vec3};
use crate::physics::player::REACH;

/// How far a player reaches to strike a creature, from the eye to its box (Minecraft's 3,
/// with a little slack for a server running behind).
pub const REACH_CREATURE: f32 = 3.6;

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
    /// The target creature is alive and within the actor's reach.
    CreatureInReach,
    /// The actor holds something it can eat.
    HoldingFood,
    /// The actor has room for food.
    Hungry,
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
    /// Restore the actor's health and return it to spawn.
    Respawn,
    /// Sow one held seed on top of the target, and grant the crop's planting experience.
    Plant,
    /// Pick a ripe crop's harvest, leaving the plant to fruit again, and grant the crop's
    /// harvest experience.
    Pick,
    /// Eat one held item, filling the actor's hunger by its food value.
    EatHeld,
    /// Craft what the actor's crafting grid holds: once onto the pointer
    /// (`ActionTarget::Slot(1)`), or as many times as it can into the inventory (`Slot(0)`).
    Craft,
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
    fn actor_eye(&self) -> Vec3;
    fn actor_alive(&self) -> bool;
    fn actor_held(&self) -> Option<BlockId>;
    /// The actor's hunger points (see [`crate::vitals`]); full unless a context keeps it.
    fn actor_hunger(&self) -> u8 {
        crate::vitals::MAX_HUNGER
    }
    /// Whether any agent's body overlaps the cell.
    fn occupied(&self, cell: IVec3) -> bool;
    /// The box of a living creature, if there is one with that id.
    fn creature_box(&self, _id: u32) -> Option<crate::physics::Aabb> {
        None
    }
}

/// Where a block placed against `face` of `block` goes: into `block` itself when it can
/// be replaced (aiming at grass puts the block on the ground beneath it), otherwise into
/// the cell in front of the face.
pub fn placement_cell(ctx: &impl ActionContext, block: IVec3, face: Face) -> IVec3 {
    if ctx.block(block).is_some_and(|b| ctx.info(b).replaceable) { block } else { block + face.normal() }
}

/// The blocks resting on `pos` that go when it is cleared: the plants standing on it (a
/// flower, both halves of tall grass, a stalk of sugar cane) and the column hanging from
/// it (vines, roots, a stalactite).
pub fn supported_by(ctx: &impl ActionContext, pos: IVec3) -> Vec<IVec3> {
    let column = |step: i32, support: Support| {
        (1..).map(move |d| pos + IVec3::Y * (d * step)).take_while(move |&p| ctx.block(p).is_some_and(|b| ctx.info(b).support == support))
    };
    column(1, Support::Below).chain(column(-1, Support::Above)).collect()
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
            Requirement::HoldingSeed => ctx.actor_held().and_then(|b| ctx.crops().planted_by(b)).is_some(),
            Requirement::Plantable => {
                let (block, face) = block_target()?;
                let Some(crop) = ctx.actor_held().and_then(|b| ctx.crops().planted_by(b)) else { return Ok(false) };
                let ground = ctx.block(block).map(|b| ctx.blocks().def(b));
                let fits = ground.is_some_and(|def| ctx.crops().get(crop).def.ground.iter().any(|tag| def.has_tag(tag)));
                let room = ctx.block(block + IVec3::Y).is_some_and(|b| b.is_air() || (ctx.info(b).replaceable && ctx.info(b).fluid.is_none()));
                face == Face::PosY && fits && room
            }
            Requirement::HoldingFood => ctx.actor_held().is_some_and(|b| ctx.info(b).food > 0),
            Requirement::Hungry => ctx.actor_hunger() < crate::vitals::MAX_HUNGER,
            Requirement::SkillLevel => match ctx.actor_held().and_then(|b| ctx.crops().planted_by(b)) {
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
            Requirement::CreatureInReach => {
                let ActionTarget::Creature(id) = target else { return Err(Rejection::WrongTarget) };
                ctx.creature_box(id).is_some_and(|aabb| {
                    let nearest = ctx.actor_eye().clamp(aabb.min, aabb.max);
                    ctx.actor_eye().distance(nearest) <= REACH_CREATURE
                })
            }
            Requirement::Replaceable => {
                let (block, face) = block_target()?;
                ctx.block(placement_cell(ctx, block, face)).is_some_and(|b| ctx.info(b).replaceable)
            }
            Requirement::Unobstructed => {
                let (block, face) = block_target()?;
                let solid = ctx.actor_held().is_some_and(|b| ctx.info(b).solid);
                !solid || !ctx.occupied(placement_cell(ctx, block, face))
            }
            Requirement::Supported => {
                let (block, face) = block_target()?;
                let cell = placement_cell(ctx, block, face);
                let solid = |p: IVec3| ctx.block(p).is_some_and(|b| ctx.info(b).solid);
                match ctx.actor_held().map_or(Support::None, |b| ctx.info(b).support) {
                    Support::None => true,
                    Support::Below => solid(cell - IVec3::Y),
                    Support::Above => solid(cell + IVec3::Y),
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
                ActionTarget::Block { block, .. } => ctx.block(block).map_or(0.0, |b| ctx.info(b).hardness.max(0.0)),
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
        ])
        .unwrap();
        let blocks = [(IVec3::ZERO, STONE), (IVec3::Y, FLOWER), (IVec3::Z, STONE)].into_iter().collect();
        World { registry, blocks, held, crops: CropRegistry::default() }
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
