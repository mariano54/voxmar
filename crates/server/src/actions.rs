//! The action executor: propose, validate, schedule one completion, then commit
//! atomically or cancel. Long actions (mining) are a single scheduled event, not work
//! done every tick.
//!
//! A player's client times their action too, and shows it done when its own clock says so
//! (a broken block vanishes at once). When the server runs behind (a busy machine, or
//! ticks skipped to catch up), its completion comes later than the client's, and the
//! client's next proposal (the block behind) must not cancel an action the player saw
//! finish. The client says when it shows one done (`Finished`): if most of its time has
//! passed by the server's clock it completes then, as Minecraft accepts a block broken once
//! most of its time has passed; otherwise it completes in its own time, whatever the
//! player starts next. A server stalled when the proposal came (streaming, a slow tick)
//! starts its clock late, and judged by that clock alone the break was lost.

use std::time::Instant;

use rustc_hash::FxHashMap;
use voxmar_engine::action::{ActionContext, ActionDef, ActionId, ActionTarget, Effect, placement_cell, supported_by};
use voxmar_engine::block::{BlockId, BlockInfo, BlockRegistry};
use voxmar_engine::crop::CropRegistry;
use voxmar_engine::skill::SkillId;
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::schedule::{EventId, Scheduler};
use voxmar_engine::traits;
use voxmar_protocol::{PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::player::MAX_HEALTH;

/// An action waiting for its completion tick.
pub struct Pending {
    agent: PlayerId,
    action: ActionId,
    target: ActionTarget,
}

/// Share of a timed action's time that must have passed by the clock for a new proposal
/// to complete it rather than cancel it (Minecraft's 0.7 for breaking blocks).
const MOSTLY_DONE: f32 = 0.7;

#[derive(Default)]
pub struct Executor {
    scheduler: Scheduler<Pending>,
    /// Each agent performs at most one timed action at a time: its event, when it started
    /// by the clock, and how long it takes.
    in_progress: FxHashMap<PlayerId, (EventId, Instant, f32)>,
    /// Actions proposed so far, for `Lag::proposal_stall`.
    proposals: u64,
}

/// Validation view of the world from one agent's point of view.
struct Context<'a> {
    server: &'a Server,
    agent: PlayerId,
}

impl ActionContext for Context<'_> {
    fn block(&self, pos: IVec3) -> Option<BlockId> {
        self.server.block(pos)
    }

    fn info(&self, block: BlockId) -> &BlockInfo {
        self.server.content.blocks.info(block)
    }

    fn blocks(&self) -> &BlockRegistry {
        &self.server.content.blocks
    }

    fn crops(&self) -> &CropRegistry {
        &self.server.content.crops
    }

    fn actor_level(&self, skill: SkillId) -> u8 {
        self.server.content.skills.level(self.server.players[&self.agent].skills.xp(skill))
    }

    fn actor_eye(&self) -> Vec3 {
        self.server.players[&self.agent].eye()
    }

    fn actor_alive(&self) -> bool {
        self.server.players[&self.agent].health.alive()
    }

    fn actor_held(&self) -> Option<BlockId> {
        self.server.players[&self.agent].held()
    }

    fn actor_hunger(&self) -> u8 {
        self.server.players[&self.agent].vitals.hunger
    }

    fn occupied(&self, cell: IVec3) -> bool {
        self.server.players.values().any(|p| p.joined && p.health.alive() && p.aabb().intersects(|c| c == cell))
    }

    fn creature_box(&self, id: u32) -> Option<voxmar_engine::physics::Aabb> {
        self.server.creatures.reach_box(id)
    }
}

impl Server {
    fn action_def(&self, action: ActionId) -> Option<ActionDef> {
        self.content.actions.get(action).cloned()
    }

    pub(crate) fn propose(&mut self, agent: PlayerId, action: ActionId, target: ActionTarget) {
        self.executor.proposals += 1;
        let stall = self.config.lag.proposal_stall;
        if !stall.is_zero() && self.executor.proposals % 2 == 1 {
            std::thread::sleep(stall);
        }
        self.supersede_action(agent);
        let Some(def) = self.action_def(action) else { return };
        let context = Context { server: self, agent };
        if def.validate(&context, target).is_err() {
            self.reject(agent, target);
            return;
        }
        let seconds = def.seconds(&context, target);
        let ticks = (seconds * TICKS_PER_SECOND as f32).round() as u64;
        if ticks == 0 {
            self.commit(agent, &def, target);
        } else {
            let id = self.executor.scheduler.schedule(self.tick + ticks, Pending { agent, action, target });
            self.executor.in_progress.insert(agent, (id, Instant::now(), seconds));
        }
    }

    pub(crate) fn cancel_action(&mut self, agent: PlayerId) {
        if let Some((id, ..)) = self.executor.in_progress.remove(&agent) {
            self.executor.scheduler.cancel(id);
        }
    }

    /// The player's client shows the action in progress done: it completes now if most of
    /// its time has passed by the clock, otherwise at its own completion tick, no longer
    /// in the way of (or cancelled by) the player's next action.
    pub(crate) fn finished_action(&mut self, agent: PlayerId) {
        let Some((id, started, seconds)) = self.executor.in_progress.remove(&agent) else { return };
        if started.elapsed().as_secs_f32() >= seconds * MOSTLY_DONE
            && let Some(pending) = self.executor.scheduler.cancel(id)
        {
            self.finish(pending);
        }
    }

    /// Makes way for a new action: the one in progress is completed now if most of its
    /// time has passed by the clock (the player saw it finish), otherwise cancelled.
    fn supersede_action(&mut self, agent: PlayerId) {
        let Some((id, started, seconds)) = self.executor.in_progress.remove(&agent) else { return };
        let Some(pending) = self.executor.scheduler.cancel(id) else { return };
        if started.elapsed().as_secs_f32() >= seconds * MOSTLY_DONE {
            self.finish(pending);
        }
    }

    /// Commits an action whose time is up, if its requirements still hold.
    fn finish(&mut self, pending: Pending) {
        if !self.players.contains_key(&pending.agent) {
            return;
        }
        let Some(def) = self.action_def(pending.action) else { return };
        if def.validate(&Context { server: self, agent: pending.agent }, pending.target).is_ok() {
            self.commit(pending.agent, &def, pending.target);
        } else {
            self.reject(pending.agent, pending.target);
        }
    }

    /// Completes every action due this tick. Requirements are checked again, since the
    /// world may have changed while the action was in progress.
    pub(crate) fn complete_actions(&mut self) {
        while let Some((_, id, pending)) = self.executor.scheduler.pop_due(self.tick) {
            if self.executor.in_progress.get(&pending.agent).is_some_and(|&(current, ..)| current == id) {
                self.executor.in_progress.remove(&pending.agent);
            }
            self.finish(pending);
        }
    }

    /// Applies an action's effects. Validation already guaranteed they can all apply.
    fn commit(&mut self, agent: PlayerId, def: &ActionDef, target: ActionTarget) {
        for &effect in &def.effects {
            match (effect, target) {
                (Effect::GiveDrop, ActionTarget::Block { block, .. }) => {
                    let Some(current) = self.block(block) else { continue };
                    let info = self.content.blocks.info(current);
                    let (drop, count) = (info.drops, info.drop_count);
                    if drop.is_air() || count == 0 {
                        continue;
                    }
                    let player = self.players.get_mut(&agent).unwrap();
                    player.inventory.add(drop, count);
                    player.changed(traits::INVENTORY);
                }
                (Effect::DropLoot, ActionTarget::Block { block, .. }) => {
                    let plants = supported_by(&Context { server: self, agent }, block);
                    let cells: Vec<IVec3> = std::iter::once(block).chain(plants).collect();
                    self.drop_loot(agent, &cells);
                }
                (Effect::Throw, ActionTarget::Stack { slot, count }) => self.throw(agent, slot as usize, count),
                (Effect::Plant, ActionTarget::Block { block, .. }) => self.plant(agent, block),
                (Effect::Pick, ActionTarget::Block { block, .. }) => self.pick(agent, block),
                (Effect::Strike, ActionTarget::Creature(id)) => self.player_strikes(agent, id),
                (Effect::ClearTarget, ActionTarget::Block { block, .. }) => {
                    let plants = supported_by(&Context { server: self, agent }, block);
                    for pos in std::iter::once(block).chain(plants) {
                        self.set_block(pos, BlockId::AIR);
                    }
                }
                (Effect::PlaceHeld, ActionTarget::Block { block, face }) => {
                    let cell = placement_cell(&Context { server: self, agent }, block, face);
                    let player = self.players.get_mut(&agent).unwrap();
                    let selected = player.selected;
                    let Some(item) = player.inventory.take_one(selected) else { continue };
                    player.changed(traits::INVENTORY);
                    self.set_block(cell, item);
                }
                (Effect::MoveItems, ActionTarget::Slots { from, to, count }) => {
                    let player = self.players.get_mut(&agent).unwrap();
                    if player.inventory.move_items(from as usize, to as usize, count) {
                        player.changed(traits::INVENTORY);
                    }
                }
                (Effect::QuickMove, ActionTarget::Slot(slot)) => {
                    let player = self.players.get_mut(&agent).unwrap();
                    if player.inventory.quick_move(slot as usize) {
                        player.changed(traits::INVENTORY);
                    }
                }
                (Effect::StowCursor, _) => {
                    let player = self.players.get_mut(&agent).unwrap();
                    if player.inventory.stow_cursor() {
                        player.changed(traits::INVENTORY);
                    }
                }
                (Effect::Respawn, _) => {
                    let spawn = self.spawn;
                    let player = self.players.get_mut(&agent).unwrap();
                    player.health.heal(MAX_HEALTH);
                    player.vitals.hunger = voxmar_engine::vitals::MAX_HUNGER;
                    player.position = spawn;
                    player.fall_start = None;
                    player.changed(traits::HEALTH);
                    player.changed(traits::VITALS);
                    player.connection.send(ServerMessage::Teleport { position: spawn });
                }
                (Effect::Craft, ActionTarget::Slot(times)) => {
                    let recipes = self.content.recipes.clone();
                    let player = self.players.get_mut(&agent).unwrap();
                    let recipe = |grid: &[Option<BlockId>; voxmar_engine::craft::GRID]| recipes.find(grid).map(|r| (r.output, r.count));
                    if player.inventory.craft(recipe, times == 0) > 0 {
                        player.changed(traits::INVENTORY);
                    }
                }
                (Effect::EatHeld, _) => {
                    let blocks = self.content.blocks.clone();
                    let player = self.players.get_mut(&agent).unwrap();
                    let selected = player.selected;
                    let Some(item) = player.inventory.take_one(selected) else { continue };
                    let food = blocks.info(item).food;
                    player.vitals.hunger = (player.vitals.hunger + food).min(voxmar_engine::vitals::MAX_HUNGER);
                    player.changed(traits::INVENTORY);
                    player.changed(traits::VITALS);
                }
                (effect, target) => tracing::warn!("{} cannot apply {effect:?} to {target:?}", def.name),
            }
        }
    }

    /// Tells the client the truth about anything it may have predicted.
    fn reject(&mut self, agent: PlayerId, target: ActionTarget) {
        let Some(player) = self.players.get_mut(&agent) else { return };
        player.changed(traits::INVENTORY);
        if let ActionTarget::Block { block, face } = target {
            // The target, where a block may have been placed, and plants it may have taken.
            for pos in [block, block + face.normal()].into_iter().chain((1..=3).map(|dy| block + IVec3::Y * dy)) {
                if let Some(current) = self.block(pos) {
                    self.players[&agent].connection.send(ServerMessage::BlockChanged { pos, block: current });
                }
            }
        }
    }
}
