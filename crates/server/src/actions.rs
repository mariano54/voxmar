//! The action executor: propose, validate, then commit at once (instant actions) or when
//! the action's time is up (timed ones, mining), or cancel.
//!
//! A player's client times their action too and shows it done when its own clock says so
//! (a broken block vanishes at once). Client and server must agree on every break, so the
//! server never judges a player's timed action by its own clock: it judges it by when the
//! player's messages arrived (stamped by the transport, see `voxmar_protocol`), in the
//! order they were sent. An action is due its time after its proposal arrived, and before
//! each later message from that player is handled, the player's action due by then
//! completes. A server held up (a long tick, streaming, saving) or running behind on ticks
//! therefore decides exactly what a server that kept up would have: a release before the
//! time is up cancels, a release after it is too late. Lost breaks came from each of the
//! ways the server's clock and the client's had parted before (17bf62f, f77a0ee).
//!
//! The client's clock can still run a frame ahead of the arrival clock (or the network
//! behind it), so it says when it shows the action done (`Finished`). If most of the time
//! has passed by then, as Minecraft accepts a block broken once 70% of its time has
//! passed, it completes then; otherwise in its own time. Either way nothing the player
//! does next cancels it. If the server has no such action (it refused the proposal), it
//! answers with the truth about the target: every break the client predicts is either
//! made or undone, never left standing on one side only.
//!
//! A player has at most one timed action going. A new timed proposal replaces it, and an
//! instant one (placing, throwing, an inventory move) runs alongside it.

use std::time::{Duration, Instant};

use voxmar_engine::action::{ActionContext, ActionDef, ActionId, ActionTarget, Effect, placement_cell, supported_by};
use voxmar_engine::block::{BlockId, BlockInfo, BlockRegistry};
use voxmar_engine::crop::CropRegistry;
use voxmar_engine::skill::SkillId;
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::traits;
use voxmar_protocol::{PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::player::MAX_HEALTH;

/// A timed action waiting for its time to be up.
struct Timed {
    agent: PlayerId,
    action: ActionId,
    target: ActionTarget,
    /// When its proposal arrived, and when it is due.
    started: Instant,
    due: Instant,
    /// The player's client shows it done: nothing cancels it now.
    finished: bool,
}

impl Timed {
    fn mostly_done(&self, at: Instant) -> bool {
        at.saturating_duration_since(self.started) >= self.due.duration_since(self.started).mul_f32(MOSTLY_DONE)
    }
}

/// Actions shorter than half a tick complete at once.
const INSTANT: f32 = 0.5 / TICKS_PER_SECOND as f32;

/// Share of a timed action's time that must have passed when the client shows it done (or
/// replaces it) for it to complete then (Minecraft's 0.7 for breaking blocks).
const MOSTLY_DONE: f32 = 0.7;

#[derive(Default)]
pub struct Executor {
    /// Timed actions in progress: at most one unfinished per player, and any number the
    /// player's client has shown done waiting for their time.
    timed: Vec<Timed>,
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

    fn cooking(&self) -> Option<&voxmar_engine::cooking::CookingRegistry> {
        Some(&self.server.content.cooking)
    }

    fn creature_accepts(&self, id: u32, item: BlockId) -> bool {
        self.server.creature_accepts(self.agent, id, item)
    }

    fn creature_box(&self, id: u32) -> Option<voxmar_engine::physics::Aabb> {
        self.server.creatures.reach_box(id)
    }
}

impl Server {
    fn action_def(&self, action: ActionId) -> Option<ActionDef> {
        self.content.actions.get(action).cloned()
    }

    pub(crate) fn propose(&mut self, agent: PlayerId, action: ActionId, target: ActionTarget, at: Instant) {
        self.executor.proposals += 1;
        let stall = self.config.lag.proposal_stall;
        if !stall.is_zero() && self.executor.proposals % 2 == 1 {
            std::thread::sleep(stall);
        }
        let Some(def) = self.action_def(action) else { return };
        let context = Context { server: self, agent };
        let seconds = def.seconds(&context, target);
        let timed = seconds >= INSTANT;
        if timed {
            self.supersede_action(agent, at);
        }
        let context = Context { server: self, agent };
        if def.validate(&context, target).is_err() {
            self.reject(agent, target);
        } else if !timed {
            self.commit(agent, &def, target);
        } else {
            let due = at + Duration::from_secs_f32(seconds);
            self.executor.timed.push(Timed { agent, action, target, started: at, due, finished: false });
        }
    }

    /// The player let go before the time was up: their unfinished timed action is dropped.
    pub(crate) fn cancel_action(&mut self, agent: PlayerId) {
        self.executor.timed.retain(|t| t.agent != agent || t.finished);
    }

    /// The player's client shows the timed action at `target` done (see the module docs).
    pub(crate) fn finished_action(&mut self, agent: PlayerId, target: ActionTarget, at: Instant) {
        let Some(i) = self.executor.timed.iter().position(|t| t.agent == agent && !t.finished && t.target == target) else {
            // Refused, or replaced: the client is told what is really there.
            self.reject(agent, target);
            return;
        };
        if self.executor.timed[i].mostly_done(at) {
            let timed = self.executor.timed.swap_remove(i);
            self.finish(timed.agent, timed.action, timed.target);
        } else {
            self.executor.timed[i].finished = true;
        }
    }

    /// Makes way for a new timed action: the unfinished one is completed now if most of
    /// its time has passed (the player saw it nearly done), otherwise dropped.
    fn supersede_action(&mut self, agent: PlayerId, at: Instant) {
        let Some(i) = self.executor.timed.iter().position(|t| t.agent == agent && !t.finished) else { return };
        let timed = self.executor.timed.swap_remove(i);
        if timed.mostly_done(at) {
            self.finish(timed.agent, timed.action, timed.target);
        }
    }

    /// Commits an action whose time is up, if its requirements still hold.
    fn finish(&mut self, agent: PlayerId, action: ActionId, target: ActionTarget) {
        if !self.players.contains_key(&agent) {
            return;
        }
        let Some(def) = self.action_def(action) else { return };
        if def.validate(&Context { server: self, agent }, target).is_ok() {
            self.commit(agent, &def, target);
        } else {
            self.reject(agent, target);
        }
    }

    /// Completes every timed action due by `now`, earliest first. Requirements are checked
    /// again, since the world may have changed while the action was in progress.
    pub(crate) fn complete_actions(&mut self, now: Instant) {
        self.complete_due(now, None);
    }

    /// Completes `agent`'s timed actions due by `at`, when a message of theirs that arrived
    /// then is about to be handled.
    pub(crate) fn complete_actions_of(&mut self, agent: PlayerId, at: Instant) {
        self.complete_due(at, Some(agent));
    }

    fn complete_due(&mut self, by: Instant, agent: Option<PlayerId>) {
        let is_due = |t: &Timed| t.due <= by && agent.is_none_or(|a| a == t.agent);
        if !self.executor.timed.iter().any(is_due) {
            return;
        }
        let (mut due, rest): (Vec<Timed>, Vec<Timed>) = std::mem::take(&mut self.executor.timed).into_iter().partition(is_due);
        self.executor.timed = rest;
        due.sort_by_key(|t| (t.due, t.agent));
        for timed in due {
            self.finish(timed.agent, timed.action, timed.target);
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
                (Effect::Cook, ActionTarget::Block { block, .. }) => self.cook(agent, block),
                (Effect::Tend, ActionTarget::Creature(id)) => self.tend(agent, id),
                (Effect::Inspect, ActionTarget::Creature(id)) => self.inspect(agent, id),
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
                    // Cooked food also heals at once (docs/food-and-cooking.md, decision 2).
                    let heal = blocks.def(item).tags.iter().find_map(|tag| tag.strip_prefix("heal:")?.parse::<u8>().ok()).unwrap_or(0);
                    if heal > 0 && player.health.alive() {
                        player.health.heal(heal);
                        player.changed(traits::HEALTH);
                    }
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
