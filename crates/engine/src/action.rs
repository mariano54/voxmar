//! Actions: the only way the world changes in discrete steps.
//!
//! Every action goes through one lifecycle, whoever performs it (player, machine, NPC):
//! propose, validate, schedule one completion, then commit atomically or cancel. Action
//! definitions are data; what they may require and do is a small, fixed vocabulary of
//! requirements and effects owned by the engine, which is what lets any package add
//! actions while the engine keeps every change validated, deterministic and networked.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::{BlockId, BlockInfo, RegistryError, RenderKind};
use crate::coords::Face;
use crate::glam::{IVec3, Vec3};
use crate::physics::player::REACH;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActionId(pub u16);

/// What an action is aimed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionTarget {
    None,
    Block { block: IVec3, face: Face },
    Slots { from: u8, to: u8 },
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
    /// The block next to the target face is empty (air or a fluid).
    Replaceable,
    /// No agent stands where the held block would go.
    Unobstructed,
}

/// Engine-owned effects that actions compile down to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    /// Give the actor what the target block drops.
    GiveDrop,
    /// Turn the target block into air.
    ClearTarget,
    /// Take one held item and put it against the target face.
    PlaceHeld,
    /// Move a stack between the actor's inventory slots.
    MoveStack,
    /// Restore the actor's health and return it to spawn.
    Respawn,
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
    fn actor_eye(&self) -> Vec3;
    fn actor_alive(&self) -> bool;
    fn actor_held(&self) -> Option<BlockId>;
    /// Whether any agent's body overlaps the cell.
    fn occupied(&self, cell: IVec3) -> bool;
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
            Requirement::HoldingBlock => ctx.actor_held().is_some(),
            Requirement::Replaceable => {
                let (block, face) = block_target()?;
                ctx.block(block + face.normal()).is_some_and(|b| {
                    let info = ctx.info(b);
                    b.is_air() || (info.render == RenderKind::Translucent && !info.solid)
                })
            }
            Requirement::Unobstructed => {
                let (block, face) = block_target()?;
                let solid = ctx.actor_held().is_some_and(|b| ctx.info(b).solid);
                !solid || !ctx.occupied(block + face.normal())
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
