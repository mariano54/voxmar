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
//! instant one (placing, throwing, an inventory move) runs alongside it. One shown done
//! early still runs to its time, so the next one's time starts only when it is due: a
//! client that shows breaks done early never mines several blocks side by side.

use std::time::{Duration, Instant};

use voxmar_engine::action::{ActionContext, ActionDef, ActionId, ActionTarget, Effect, Rejection, Requirement, placed_block, placement_cell, supported_by};
use voxmar_engine::block::{BlockId, BlockInfo, BlockRegistry};
use voxmar_engine::combat::Slot;
use voxmar_engine::crop::CropRegistry;
use voxmar_engine::skill::SkillId;
use voxmar_engine::glam::{IVec2, IVec3, Vec3};
use voxmar_engine::traits;
use voxmar_engine::lang::Words;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;

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
    /// The hotbar slot in hand, what was in it and how worn: what the work wears. When it
    /// began, or for a break shown done with the same kind of tool in hand, then.
    tool: Tool,
    /// For a break, what the player held when their client showed it done: what it is
    /// judged by at its time, and draws its tool's bonus with, whatever they take in hand
    /// meanwhile (the next block's tool). Anything else (a bite, a cook, a cast) uses what
    /// is in hand when it completes, so it is judged by that.
    held: Option<Option<BlockId>>,
}

/// A hotbar slot, what was in it and its wear, when an action began.
pub(crate) type Tool = (usize, Option<voxmar_engine::block::BlockId>, u16);

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

impl Server {
    /// A move put on or took off a charm (`was` the one worn before): its effect is judged
    /// at once, not at the next once-a-second pass.
    fn charm_changed(&mut self, agent: PlayerId, was: Option<BlockId>) {
        if self.players.get(&agent).is_some_and(|p| p.inventory.worn[Slot::Charm.index()].map(|s| s.item) != was) {
            crate::process::Survival::refresh(self, agent);
        }
    }

    /// Whether `player` is part way through a timed action that eats (a bite of food). A bite
    /// the client has shown done early still runs to its time, so it still counts.
    pub(crate) fn eating(&self, player: PlayerId) -> bool {
        self.executor.timed.iter().any(|t| {
            t.agent == player && self.content.actions.get(t.action).is_some_and(|def| def.effects.contains(&Effect::EatHeld))
        })
    }
}

/// The validation view of the world from `agent`'s point of view.
pub(crate) fn context(server: &Server, agent: PlayerId) -> impl ActionContext + '_ {
    Context { server, agent, held: None }
}

/// Validation view of the world from one agent's point of view.
struct Context<'a> {
    server: &'a Server,
    agent: PlayerId,
    /// What the agent is taken to hold in place of what is in their hand now.
    held: Option<Option<BlockId>>,
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
        self.server.check_level(self.agent, skill)
    }

    fn actor_own_level(&self, skill: SkillId) -> u8 {
        self.server.content.skills.level(self.server.players[&self.agent].skills.xp(skill))
    }

    fn actor_eye(&self) -> Vec3 {
        self.server.players[&self.agent].eye()
    }

    fn actor_alive(&self) -> bool {
        self.server.players[&self.agent].health.alive()
    }

    fn actor_held(&self) -> Option<BlockId> {
        self.held.unwrap_or_else(|| self.server.players[&self.agent].held())
    }

    fn actor_pouched(&self) -> Vec<BlockId> {
        self.server.players[&self.agent].inventory.pouched.iter().map(|&(item, _)| item).collect()
    }

    fn actor_carried(&self) -> Vec<BlockId> {
        self.server.players[&self.agent].inventory.slots.iter().flatten().map(|s| s.item).collect()
    }

    fn actor_hurt(&self) -> bool {
        let health = self.server.players[&self.agent].health;
        health.current < health.max
    }

    fn actor_mining_speed(&self) -> f32 {
        self.server.players[&self.agent].modifiers.mining
    }

    fn occupied(&self, cell: IVec3) -> bool {
        self.server.player_in(cell)
    }

    fn creature_in(&self, cell: IVec3) -> bool {
        self.server.creature_in(cell)
    }

    fn cooking(&self) -> Option<&voxmar_engine::cooking::CookingRegistry> {
        Some(&self.server.content.cooking)
    }

    fn tools(&self) -> Option<&voxmar_engine::tool::ToolRegistry> {
        Some(&self.server.content.tools)
    }

    fn flames(&self) -> Option<&voxmar_engine::flame::Flames> {
        Some(&self.server.content.flames)
    }

    fn machines(&self) -> Option<&voxmar_engine::machine::MachineRegistry> {
        Some(&self.server.content.machines)
    }

    fn bees(&self) -> Option<&voxmar_engine::bees::Bees> {
        Some(&self.server.content.bees)
    }
    fn stations(&self) -> Option<&voxmar_engine::station::Stations> {
        Some(&self.server.content.stations)
    }
    fn traps(&self) -> Option<&voxmar_engine::trap::Traps> {
        Some(&self.server.content.traps)
    }

    fn creature_accepts(&self, id: u32, item: BlockId) -> bool {
        self.server.creature_accepts(self.agent, id, item)
    }

    fn creature_is_mine(&self, id: u32) -> bool {
        self.server.keeps(self.agent, id)
    }

    fn creature_box(&self, id: u32) -> Option<voxmar_engine::physics::Aabb> {
        self.server.creatures.reach_box(id, self.server.content.combat.feel.reach_lag)
    }

    fn combat(&self) -> Option<&voxmar_engine::combat::CombatRegistry> {
        Some(&self.server.content.combat)
    }
}

impl Server {
    fn action_def(&self, action: ActionId) -> Option<ActionDef> {
        self.content.actions.get(action).cloned()
    }

    pub(crate) fn propose(&mut self, agent: PlayerId, action: ActionId, target: ActionTarget, at: Instant) {
        self.executor.proposals += 1;
        let stall = self.config.lag.proposal_stall;
        if !stall.is_zero() && self.executor.proposals % 2 == 1 && self.lagging.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(stall);
        }
        let Some(def) = self.action_def(action) else { return };
        let context = Context { server: self, agent, held: None };
        let seconds = def.seconds(&context, target);
        let timed = seconds >= INSTANT;
        if timed {
            self.supersede_action(agent, at);
        }
        let context = Context { server: self, agent, held: None };
        if let Err(why) = def.validate(&context, target) {
            tracing::debug!("player {agent}: {} refused at {target:?}: {why:?}", def.name);
            self.unforeseen(agent, why);
            self.reject(agent, target);
        } else if !timed {
            let tool = self.tool_in_hand(agent);
            self.commit(agent, &def, target, None, tool);
        } else {
            if def.effects.contains(&Effect::EatHeld) {
                self.bite_started(agent);
            }
            // After any shown done early that is not yet due: one at a time.
            let started = self.executor.timed.iter().filter(|t| t.agent == agent).map(|t| t.due).fold(at, Instant::max);
            let due = started + Duration::from_secs_f32(seconds);
            let tool = self.tool_in_hand(agent);
            self.executor.timed.push(Timed { agent, action, target, started, due, finished: false, held: None, tool });
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
            self.finish(&timed);
        } else {
            let mining = self.content.actions.get(self.executor.timed[i].action).is_some_and(|def| def.duration == voxmar_engine::action::Duration::TargetHardness);
            let held = mining.then(|| self.players[&agent].held());
            let tool = self.tool_in_hand(agent);
            let timed = &mut self.executor.timed[i];
            (timed.finished, timed.held) = (true, held);
            // The same kind of tool as set its time, in hand now (perhaps another copy of
            // it): that one wears. Anything else keeps the one that set the time.
            if mining && tool.1 == timed.tool.1 {
                timed.tool = tool;
            }
        }
    }

    /// Makes way for a new timed action: the unfinished one is completed now if most of
    /// its time has passed (the player saw it nearly done), otherwise dropped.
    fn supersede_action(&mut self, agent: PlayerId, at: Instant) {
        let Some(i) = self.executor.timed.iter().position(|t| t.agent == agent && !t.finished) else { return };
        let timed = self.executor.timed.swap_remove(i);
        if timed.mostly_done(at) {
            self.finish(&timed);
        }
    }

    /// Commits an action whose time is up, if its requirements still hold: a break with
    /// what the player held when their client showed it done, if it did (`Timed::held`).
    fn finish(&mut self, timed: &Timed) {
        let (agent, target) = (timed.agent, timed.target);
        if !self.players.contains_key(&agent) {
            return;
        }
        let Some(def) = self.action_def(timed.action) else { return };
        let held = timed.held;
        match def.validate(&Context { server: self, agent, held }, target) {
            Ok(()) => self.commit(agent, &def, target, held, timed.tool),
            Err(why) => {
                self.unforeseen(agent, why);
                self.reject(agent, target);
            }
        }
    }

    /// Tells the player why, for a refusal their client could not foresee: it sees no other
    /// player's or creature's body, so it lets clay to a panel someone leans on through.
    fn unforeseen(&self, agent: PlayerId, why: Rejection) {
        if why == Rejection::Unmet(Requirement::CoatClear) {
            self.refuse(agent, Words::key("refusal.someone_there"));
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
            self.finish(&timed);
        }
    }

    /// The hotbar slot `agent` has in hand and what is in it.
    pub(crate) fn tool_in_hand(&self, agent: PlayerId) -> Tool {
        self.players.get(&agent).map_or((0, None, 0), |p| (p.selected, p.held(), p.inventory.get(p.selected).map_or(0, |s| s.wear)))
    }

    /// Applies an action's effects, as `agent`'s doing (`fire_origin`, `changed_by`) whenever
    /// it commits: as their message arrives or later, when it falls due in the run loop.
    /// `held`, if given, is what the agent is taken to hold (`Timed::held`), and `tool` the
    /// slot in hand and what was in it, what the work wears (`Timed::tool`).
    fn commit(&mut self, agent: PlayerId, def: &ActionDef, target: ActionTarget, held: Option<Option<BlockId>>, tool: Tool) {
        let by = self.players.get(&agent).map(|p| crate::player_key(&p.name));
        let fire_was = std::mem::replace(&mut self.fire_origin, by.clone());
        let changed_was = std::mem::replace(&mut self.changed_by, by);
        self.commit_effects(agent, def, target, held, tool);
        self.fire_origin = fire_was;
        self.changed_by = changed_was;
    }

    /// Applies an action's effects. Validation already guaranteed they can all apply.
    fn commit_effects(&mut self, agent: PlayerId, def: &ActionDef, target: ActionTarget, held: Option<Option<BlockId>>, tool: Tool) {
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
                    // What a player put there teaches, counts and forages nothing: taken
                    // before `picked` and `reward_break` clear the mark.
                    let plants = supported_by(&Context { server: self, agent, held: None }, block);
                    let placed: Vec<IVec3> = std::iter::once(block).chain(plants.iter().copied()).filter(|p| self.placed.contains(p)).collect();
                    if let Some(broken) = self.block(block).filter(|_| !placed.contains(&block)) {
                        self.discover_broken(agent, broken);
                    }
                    self.picked(agent, block);
                    let instead = self.reward_break(agent, block, held);
                    // A block that gave something else instead (gravel's flint) drops nothing of its own.
                    let cells: Vec<IVec3> = std::iter::once(block).filter(|_| !instead).chain(plants).collect();
                    self.drop_loot(agent, &cells, &placed);
                }
                (Effect::Throw, ActionTarget::Stack { slot, count }) => self.throw(agent, slot as usize, count),
                (Effect::Plant, ActionTarget::Block { block, .. }) => self.plant(agent, block),
                (Effect::Pick, ActionTarget::Block { block, .. }) => self.pick(agent, block),
                (Effect::Strike, ActionTarget::Creature(id)) => self.player_strikes(agent, id),
                (Effect::Dodge, _) => self.dodge(agent),
                (Effect::Cook, ActionTarget::Block { block, .. }) => self.cook(agent, block),
                (Effect::UseStation, ActionTarget::Block { block, .. }) => self.use_station(agent, block),
                (Effect::TakeFromTrap, ActionTarget::Block { block, .. }) => self.take_from_trap(agent, block),
                (Effect::Fertilise, ActionTarget::Block { block, .. }) => self.fertilise(agent, block),
                (Effect::Tend, ActionTarget::Creature(id)) => self.tend(agent, id),
                (Effect::Inspect, ActionTarget::Creature(id)) => self.inspect(agent, id),
                (Effect::Release, ActionTarget::Creature(id)) => self.release(agent, id),
                (Effect::ClearTarget, ActionTarget::Block { block, .. }) => {
                    let plants = supported_by(&Context { server: self, agent, held: None }, block);
                    for pos in std::iter::once(block).chain(plants) {
                        self.set_block(pos, BlockId::AIR);
                    }
                }
                (Effect::PlaceHeld, ActionTarget::Block { block, face }) => {
                    let cell = placement_cell(&Context { server: self, agent, held: None }, block, face);
                    let player = self.players.get_mut(&agent).unwrap();
                    let selected = player.selected;
                    let wear = player.inventory.get(selected).map_or(0, |s| s.wear);
                    let Some(item) = player.inventory.take_one(selected) else { continue };
                    player.changed(traits::INVENTORY);
                    let placed = placed_block(&Context { server: self, agent, held: None }, item, block, face);
                    self.set_block(cell, placed);
                    // A light picked up burning burns on from where it was.
                    self.set_down_burnt(cell, item, wear);
                    // Its upper half goes in above with it (a door's top).
                    if let Some(upper) = self.content.blocks.upper(placed) {
                        self.set_block(cell + IVec3::Y, upper);
                    }
                    if self.marks_placed(item, placed) {
                        self.placed.insert(cell);
                    }
                    self.placed_plant(item, cell);
                    self.placed_machine(agent, cell);
                    self.placed_by_field(agent, item, cell);
                }
                (Effect::MoveItems, ActionTarget::Slots { from, to, count }) => {
                    // An open store's slots as it holds them now, and what moved written back.
                    self.pull_store(agent);
                    let player = self.players.get_mut(&agent).unwrap();
                    let charm = player.inventory.worn[Slot::Charm.index()].map(|s| s.item);
                    if player.inventory.move_items(from as usize, to as usize, count) {
                        player.changed(traits::INVENTORY);
                        self.push_store(agent);
                        self.charm_changed(agent, charm);
                    }
                }
                (Effect::QuickMove, ActionTarget::Slot(slot)) => {
                    self.pull_store(agent);
                    let player = self.players.get_mut(&agent).unwrap();
                    let charm = player.inventory.worn[Slot::Charm.index()].map(|s| s.item);
                    if player.inventory.quick_move(slot as usize) {
                        player.changed(traits::INVENTORY);
                        self.push_store(agent);
                        self.charm_changed(agent, charm);
                    }
                }
                (Effect::StowCursor, _) => {
                    let player = self.players.get_mut(&agent).unwrap();
                    if player.inventory.stow_cursor() {
                        player.changed(traits::INVENTORY);
                    }
                    // The screen closed: so does the store.
                    self.close_store(agent);
                }
                (Effect::Respawn, _) => {
                    // Home, if the player has one, else the bed they last slept in, else
                    // world spawn. What they carried is left where they fell first.
                    self.bury_if_dead(agent);
                    let player = self.players.get_mut(&agent).unwrap();
                    let spawn = player.home_spot(&self.lit).unwrap_or(self.spawn);
                    player.buried = false;
                    player.health.heal(u8::MAX);
                    // Where they wake is where a grave goes until they stand elsewhere,
                    // not where they last stood in the life before.
                    player.last_ground = Some(spawn);
                    player.fall_start = None;
                    player.changed(traits::HEALTH);
                    player.changed(traits::VITALS);
                    player.teleport(spawn);
                    // Alive again: effects judged afresh, so none of the last life's (a
                    // charm left in the grave) outlasts it.
                    crate::process::Survival::refresh(self, agent);
                }
                (Effect::Craft, ActionTarget::Slot(times)) => self.craft(agent, times == 0),
                (Effect::LayRecipe, ActionTarget::Recipe { first, ways, count }) => self.lay_recipe(agent, first as usize, ways as usize, count),
                (Effect::Fish, ActionTarget::Block { block, .. }) => self.fish(agent, block),
                (Effect::Relight, ActionTarget::Block { block, .. }) => self.relight(agent, block),
                (Effect::Swap, ActionTarget::Block { block, .. }) => {
                    // Opened or shut here first: the clients follow what the server holds.
                    let changes = voxmar_engine::action::swapping(&Context { server: self, agent, held: None }, block);
                    // A swap is the same thing in another state: a player's door or curtain
                    // stays theirs (it gives itself back when broken), a wild one stays wild.
                    let placed: Vec<IVec3> = changes.iter().map(|&(pos, _)| pos).filter(|pos| self.placed.contains(pos)).collect();
                    self.set_blocks(&changes);
                    self.placed.extend(placed);
                }
                (Effect::WearHeld, ActionTarget::Block { block, .. }) => self.wear_on_block(agent, block, def.effects.contains(&Effect::Till), tool),
                (Effect::Coat, ActionTarget::Block { block, .. }) => self.coat(agent, block),
                (Effect::Till, ActionTarget::Block { block, .. }) => {
                    if let Some(farmland) = self.content.blocks.id("base:farmland") {
                        self.set_block(block, farmland);
                    }
                }
                (Effect::EmptyPouch, _) => {
                    let player = self.players.get_mut(&agent).unwrap();
                    if let Some(pouch) = player.held().filter(|&p| self.content.blocks.def(p).has_tag("pouch")) {
                        player.inventory.empty_pouch(pouch);
                        player.changed(traits::INVENTORY);
                    }
                }
                (Effect::Light, ActionTarget::Block { block, .. }) => self.light(agent, block),
                (Effect::UseMachine, ActionTarget::Block { block, .. }) => self.use_machine(agent, block),
                (Effect::OpenStore, ActionTarget::Block { block, .. }) => self.open_store(agent, block),
                (Effect::PickLock, ActionTarget::Block { block, .. }) => self.pick_lock(agent, block),
                (Effect::TendBees, ActionTarget::Block { block, .. }) => self.tend_bees(agent, block),
                (Effect::Pray, ActionTarget::Block { .. }) => self.pray_at_altar(agent),
                (Effect::Sleep, ActionTarget::Block { block, .. }) => self.sleep(agent, block),
                (Effect::Sit, ActionTarget::Block { block, .. }) => self.sit(agent, block),
                (Effect::UseTable, ActionTarget::Block { block, .. }) => {
                    self.players.get_mut(&agent).unwrap().station = Some(block);
                }
                (Effect::EatHeld, _) => {
                    let (blocks, effects) = (self.content.blocks.clone(), self.content.effects.clone());
                    let most = self.survival.meal_most;
                    let player = self.players.get_mut(&agent).unwrap();
                    let selected = player.selected;
                    let Some(item) = player.inventory.take_one(selected) else { continue };
                    // Quests count what was eaten (`Ate`).
                    *player.tallies.entry(format!("ate:{}", blocks.def(item).name)).or_default() += 1;
                    // Food heals, the same food a little less each time in a row.
                    let share = player.recent_food.share(item);
                    player.recent_food.eat(item);
                    // A pantry or larder at home makes more of a meal (effects.ron's meal_heal, meal_time),
                    // all together up to survival's meal_most.
                    let (meal_heal, meal_time) = (player.modifiers.meal_heal.min(most), player.modifiers.meal_time.min(most));
                    let heal = (blocks.info(item).food as f32 * share * meal_heal).round().min(u8::MAX as f32) as u8;
                    // What it does besides (content's effects.ron): hearty, queasy and so on.
                    let mut given = effects.given_by(&blocks.def(item).tags);
                    // Only good effects last longer: raw meat's queasiness does not.
                    given.iter_mut().filter(|(effect, _)| effects.get(*effect).kind == voxmar_engine::effect::Kind::Buff).for_each(|(_, seconds)| *seconds *= meal_time);
                    player.give_effects(&given);
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

    /// `agent` sleeps at the bed at `bed`: it is where they wake after dying (their hearth
    /// first, once they have one), and they wake rested, as well as the home around the
    /// bed allows (effects.ron's `sleeps`; the better rests only by night). A bed a village
    /// home was furnished with is its resident's and turns them away, but in creative. A bed
    /// always works, as a bed; the rest is held back while it or a better one is still on
    /// them, or the last sleep's mend still runs (`EffectRegistry::rested`): a nap's lesser
    /// rest does not keep a night's better one away, and a bed after a bed mends no more.
    fn sleep(&mut self, agent: PlayerId, bed: IVec3) {
        // A bed a village furnished a home with (its settlement's plan says so, and no player
        // has set one down there since) is its resident's; any other works, however it came
        // to be (placed, /setblock, an older save). In creative nothing stops them.
        let creative = self.players.get(&agent).is_some_and(|p| p.allow_flight);
        if !creative && !self.placed.contains(&bed) && self.planned_bed(bed) {
            self.refuse(agent, Words::key("bed.someones"));
            return;
        }
        let player = self.players.get_mut(&agent).unwrap();
        player.bed = Some(bed.as_vec3() + Vec3::new(0.5, 1.0, 0.5));
        let key = if player.home.is_some() { "bed.set_home" } else { "bed.set" };
        player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key(key) });
        // Claimed all the same: only the rest waits.
        if let Some(why) = self.unsafe_to_sleep(agent) {
            self.refuse(agent, Words::key(why));
            return;
        }
        let effects = self.content.effects.clone();
        let home = self.home_survey(agent);
        let feet = self.players[&agent].position.floor().as_ivec3();
        let outdoors = self.open_sky(feet + IVec3::Y * 2);
        let calm = voxmar_engine::weather::Weather::NONE;
        let situation = voxmar_engine::effect::Situation { felt: 0.0, warmed: false, outdoors, weather: &calm, biome: "", biome_tags: &[], home: home.as_ref(), done: &self.players[&agent].quests_done, held: &[], near: &[], worn: &[], seated: false, night: self.night() };
        // A proper bed rests better than a bag on the ground (effects.ron's `bed` tags).
        let bed_tags = self.block(bed).map(|b| self.content.blocks.def(b).tags.clone()).unwrap_or_default();
        let given = effects.given_by_sleep(&situation, &bed_tags);
        // The rest is held back while it, or a better one, is still on them; a nap's lesser
        // rest does not keep a night's better one away.
        if effects.rested(&self.players[&agent].timed_effects, &given) {
            let player = &self.players[&agent];
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("bed.not_tired") });
            return;
        }
        // The best rest it gives (the one the others are hidden by) is the one it says.
        let best = effects.best_rest(&given);
        // By day, the better rest a night's sleep here would give (effects.ron's `night`).
        let at_night = if situation.night { None } else { effects.best_rest(&effects.given_by_sleep(&voxmar_engine::effect::Situation { night: true, ..situation }, &bed_tags)).filter(|&night| Some(night) != best) };
        let player = self.players.get_mut(&agent).unwrap();
        player.give_effects(&given);
        player.changed(traits::VITALS);
        if let Some(best) = best {
            let def = effects.get(best);
            let key = voxmar_engine::lang::def_key("effect", &def.name);
            let words = Words::key("bed.rested").with("name", Words::Own(key.clone(), def.title.clone())).with("what", Words::Own(format!("{key}.description"), def.description.clone()));
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
        }
        if let Some(night) = at_night {
            let def = effects.get(night);
            let key = voxmar_engine::lang::def_key("effect", &def.name);
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("bed.night_rest").with("name", Words::Own(key, def.title.clone())) });
        }
    }

    /// Whether a settlement's plan furnished one of its homes with a bed at `bed`, and that
    /// very bed still stands there: one set down in its place (a bag, another bed, by hand
    /// or by command) is not the village's.
    fn planned_bed(&self, bed: IVec3) -> bool {
        let Some(now) = self.block(bed) else { return false };
        self.content.blocks.def(now).has_tag("bed")
            && self.overworld.settlement_holding(IVec2::new(bed.x, bed.z)).is_some_and(|s| s.buildings.iter().any(|b| b.beds.contains(&bed)))
            && voxmar_worldgen::Generator::generate(&*self.overworld, voxmar_engine::coords::ColumnPos::of_block(bed)).block(bed) == now
    }

    /// Why `agent` cannot lie down now, if they cannot: a creature that hunts players (but
    /// not one they keep: it hunts everyone else) within `sleep_threat_reach` blocks, or
    /// wounded within the last `sleep_after_hurt` seconds (`survival.ron`). No sleeping off a
    /// fight for the rest it gives. Nothing hunts one in creative, so nothing stops them.
    pub(crate) fn unsafe_to_sleep(&self, agent: PlayerId) -> Option<&'static str> {
        let player = self.players.get(&agent)?;
        if player.allow_flight {
            return None;
        }
        let reach = self.survival.sleep_threat_reach;
        let hunted = self.creatures.live.values().any(|c| {
            c.health > 0 && !c.companion && c.owner.as_deref() != Some(player.name.as_str()) && c.body.position.distance(player.position) <= reach
                && self.content.creatures.get(c.species).behaviours.iter().any(|b| matches!(b.behaviour, voxmar_engine::creature::Behaviour::Hunt { .. }))
        });
        let after = (self.survival.sleep_after_hurt * voxmar_protocol::TICKS_PER_SECOND as f32).round() as u64;
        if hunted {
            Some("bed.threatened")
        } else if player.hurt_at.is_some_and(|at| self.tick < at + after) {
            Some("bed.just_hurt")
        } else {
            None
        }
    }

    /// `agent` sits on the seat at `seat`: set on it (on top of a stool, in a cushion),
    /// facing as they were, their eye lowered (`vitals::eye`), until they move off it.
    fn sit(&mut self, agent: PlayerId, seat: IVec3) {
        let solid = self.block(seat).is_some_and(|b| self.content.blocks.info(b).solid);
        let player = self.players.get_mut(&agent).unwrap();
        player.teleport(voxmar_engine::vitals::seated_feet(seat, solid));
        player.vitals.seated = Some(seat);
        player.changed(traits::VITALS);
        // What sitting there gives (a table's meal bonus) holds from the first bite.
        crate::process::Survival::refresh(self, agent);
    }

    /// Crafts what `agent`'s grid holds, once or (`all`) as many times as it can: if the
    /// recipe's skill level is met, for a recipe bigger than two by two, at a crafting
    /// table within reach, and for one made near a block, within reach of one. Experience
    /// comes for every one made.
    fn craft(&mut self, agent: PlayerId, all: bool) {
        let recipes = self.content.recipes.clone();
        let allowed = self.allowed_recipes(agent);
        let near = self.near_tags(agent);
        let near = |tag: &str| near.iter().any(|t| t == tag);
        let player = self.players.get_mut(&agent).unwrap();
        // A worn tool and what mends it, by the block it is mended at, mend it, at the level
        // its recipe asks.
        if let Some(mended) = self.content.tools.mended(&player.inventory.craft, near) {
            let item = mended.item;
            let content = &self.content;
            let level = |skill| {
                let boosted = voxmar_engine::level::check_level(&content.skills, &content.effects, &player.skills, &player.vitals.effects, skill);
                (content.skills.level(player.skills.xp(skill)), boosted)
            };
            let able = voxmar_engine::level::lacks_for_mend(&content.tools, item, level, &player.inventory, &content.blocks).is_none();
            if able && player.inventory.mend(mended, all) {
                let name = self.content.blocks.def(item).name.clone();
                self.tally(agent, "mended", &name, 1);
            }
            self.players.get_mut(&agent).unwrap().changed(traits::INVENTORY);
            return;
        }
        // Crafting all goes on only while the grid still makes the same recipe: experience
        // and tallies are this recipe's, once for each one made.
        let first = recipes.find_near(&player.inventory.grid(), near).filter(|r| allowed(r));
        let found = first.map(|r| (r.skill, r.xp, r.output, r.count));
        let made = player.inventory.craft(recipes.while_making(first, near), all);
        // Always: a refused craft the client predicted is undone.
        player.changed(traits::INVENTORY);
        if let Some((skill, xp, output, count)) = found.filter(|_| made > 0) {
            if let Some(skill) = skill {
                self.grant_xp(agent, skill, xp * made as f32);
            }
            // For quests that ask for things made.
            let name = self.content.blocks.def(output).name.clone();
            self.tally(agent, "crafted", &name, made * count as u32);
        }
    }

    /// Whether a cell a player set `item` down in, as `placed`, is marked as theirs: an ore
    /// teaches nothing broken again, leaves give no stick (`instead`) again, and a furnishing
    /// gives itself back (where the world placed one, it does not). (By the block placed: a
    /// held item may place as another, as a torch does.)
    pub(crate) fn marks_placed(&self, item: BlockId, placed: BlockId) -> bool {
        self.content.tools.rule(item).is_some_and(|rule| rule.xp > 0.0 || rule.instead.is_some()) || self.content.blocks.def(placed).has_tag("furnishing")
    }

    /// What breaking the block at `pos` teaches (`tools.ron`), and sometimes an extra drop:
    /// more often with the skill's level above what the block asks and a better tool. A
    /// block a player placed teaches nothing and gives nothing extra.
    /// Returns whether it gave something else instead of the block's own drop. `held`, if
    /// given, is the tool it was broken with in place of what is in hand now.
    fn reward_break(&mut self, agent: PlayerId, pos: IVec3, held: Option<Option<BlockId>>) -> bool {
        let Some(block) = self.block(pos) else { return false };
        let tools = self.content.tools.clone();
        let Some(rule) = tools.rule(block) else { return false };
        if self.placed.remove(&pos) {
            return false;
        }
        let Some(player) = self.players.get_mut(&agent) else { return false };
        // By the player's count of blocks broken, and where a drop pops by the world's count
        // of drops, never by the tick it breaks on.
        let [roll, other] = player.draw(crate::save::Draw::Mine);
        let held = held.unwrap_or_else(|| player.held());
        let level = rule.skill.map_or(1, |skill| self.content.skills.level(player.skills.xp(skill)));
        let bonus = voxmar_engine::crop::level_bonus(level, rule.level, self.content.skills.max_level()) + tools.yield_bonus(held, block) + player.modifiers.finds;
        let info = self.content.blocks.info(block);
        // The pops show where each drop flies, so they draw nothing from the player's rolls.
        let [a, b, c] = self.meta.next_pop(pos, agent ^ 0xd1ce);
        // Never a furnishing's: it gives itself back only where a player placed it.
        if roll < bonus && !info.drops.is_air() && info.drop_count > 0 && !self.content.blocks.def(block).has_tag("furnishing") {
            let stack = voxmar_engine::inventory::ItemStack::new(info.drops, 1);
            let body = voxmar_engine::dropped::popped(pos, [a, b, c]);
            self.items.spawn(stack, body, voxmar_engine::dropped::PICKUP_DELAY);
        }
        if let Some(skill) = rule.skill.filter(|_| rule.xp > 0.0) {
            self.grant_xp(agent, skill, rule.xp);
        }
        match rule.instead {
            Some((item, chance)) if other < chance => {
                let body = voxmar_engine::dropped::popped(pos, [b, c, a]);
                self.items.spawn(voxmar_engine::inventory::ItemStack::new(item, 1), body, voxmar_engine::dropped::PICKUP_DELAY);
                true
            }
            _ => false,
        }
    }

    /// `agent`'s level in `skill` for a check: their own, with what their effects boost
    /// (see `voxmar_engine::level`).
    pub(crate) fn check_level(&self, agent: PlayerId, skill: SkillId) -> u8 {
        let Some(player) = self.players.get(&agent) else { return 1 };
        voxmar_engine::level::check_level(&self.content.skills, &self.content.effects, &player.skills, &player.vitals.effects, skill)
    }

    /// The tags recipes are made near (a chopping block's) that some block within `agent`'s
    /// reach has.
    pub(crate) fn near_tags(&self, agent: PlayerId) -> Vec<String> {
        let Some(player) = self.players.get(&agent) else { return Vec::new() };
        let tags = self.content.near_tags();
        voxmar_engine::craft::tags_near(&tags, &self.content.blocks, player.eye(), |pos| self.block(pos))
    }

    /// Whether `agent` is at the crafting table they last opened, still within reach.
    pub(crate) fn at_table(&self, agent: PlayerId) -> bool {
        let Some(player) = self.players.get(&agent) else { return false };
        player.station.is_some_and(|at| {
            let table = self.block(at).is_some_and(|b| self.content.blocks.def(b).has_tag("crafting_table"));
            table && player.eye().distance(at.as_vec3() + 0.5) <= voxmar_engine::physics::player::REACH + 1.5
        })
    }

    /// Tells the client the truth about anything it may have predicted.
    pub(crate) fn reject(&mut self, agent: PlayerId, target: ActionTarget) {
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

#[cfg(test)]
mod sleep_tests {
    use super::*;

    /// A wolf hunts players: wild, or kept by another player, it keeps one from sleeping; kept
    /// by the sleeper, it does not, and in creative nothing does.
    #[test]
    fn only_a_hunter_one_keeps_lets_one_sleep() {
        let mut server = crate::test_server();
        let (_client, connection) = voxmar_protocol::local_pair();
        let mut joined = crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        (joined.joined, joined.name) = (true, "sleeper".into());
        server.players.insert(1, joined);
        let wolf = server.content.creatures.id("base:wolf").expect("wolves");
        let id = server.spawn_creature(wolf, server.spawn + Vec3::X * 4.0, 1).expect("room");
        assert_eq!(server.unsafe_to_sleep(1), Some("bed.threatened"), "a wild wolf");
        server.creatures.live.get_mut(&id).unwrap().owner = Some("someone_else".into());
        assert_eq!(server.unsafe_to_sleep(1), Some("bed.threatened"), "another's kept wolf hunts the sleeper all the same");
        server.creatures.live.get_mut(&id).unwrap().owner = Some("sleeper".into());
        assert_eq!(server.unsafe_to_sleep(1), None, "one's own kept wolf");
        server.creatures.live.get_mut(&id).unwrap().owner = None;
        server.players.get_mut(&1).unwrap().allow_flight = true;
        assert_eq!(server.unsafe_to_sleep(1), None, "nothing hunts one in creative");
    }
}

#[cfg(test)]
mod tests {
    use voxmar_engine::glam::IVec3;

    /// What breaking a block gives besides its own drop (gravel's flint instead, a bonus
    /// drop) is drawn by the player's count of blocks broken, not the tick it breaks on,
    /// and where it pops by the world's count of drops.
    #[test]
    fn a_break_rewards_by_the_count_of_breaks_not_the_tick() {
        let broken = |tick: u64| {
            let mut server = crate::test_server();
            server.meta.draw_key = 0x0d1a;
            let (_client, connection) = voxmar_protocol::local_pair();
            let mut player = crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
            (player.joined, player.draw_key) = (true, crate::player::player_draw_key(0x0d1a, "ada"));
            // Finds enough that some breaks give a bonus drop besides.
            player.modifiers.finds = 0.5;
            server.players.insert(1, player);
            let gravel = server.content.blocks.expect("base:gravel");
            let pos = server.spawn.floor().as_ivec3() + IVec3::new(2, 0, 0);
            server.tick = tick;
            let instead: Vec<bool> = (0..64)
                .map(|_| {
                    server.set_block(pos, gravel);
                    server.reward_break(1, pos, None)
                })
                .collect();
            let gravel_drops = server.content.blocks.def(server.content.blocks.info(gravel).drops).name.clone();
            let blocks = server.content.blocks.clone();
            let mut bodies: Vec<_> = server.items.saved(|item| blocks.def(item).name.clone()).into_iter().map(|d| (d.item, d.count, d.position)).collect();
            bodies.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let bonus: u32 = bodies.iter().filter(|(item, ..)| *item == gravel_drops).map(|&(_, count, _)| count as u32).sum();
            (instead, bodies, server.players[&1].draws.mines, bonus)
        };
        let first = broken(1_000);
        assert_eq!(first.2, 64, "each break counted on");
        let flint = first.0.iter().filter(|&&f| f).count();
        assert!((1..64).contains(&flint), "flint {flint} times in 64 breaks of gravel");
        assert!((1..64).contains(&first.3), "a bonus drop {} times in 64 breaks with finds", first.3);
        assert_eq!(broken(1_377), first, "the same breaks later gave otherwise");
    }

    /// Leaves a player set down are marked as theirs, so breaking them gives no stick: forty
    /// times placed and broken, never one, where a tree's leaves give one in five.
    #[test]
    fn placed_leaves_give_no_sticks() {
        let mut server = crate::test_server();
        let (_client, connection) = voxmar_protocol::local_pair();
        let mut player = crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        let leaves = server.content.blocks.expect("base:oak_leaves");
        assert!(server.content.tools.rule(leaves).and_then(|r| r.instead).is_some(), "a tree's leaves give sticks");
        let pos = server.spawn.floor().as_ivec3() + IVec3::new(2, 0, 0);
        let sticks = (0..40)
            .filter(|_| {
                server.set_block(pos, leaves);
                if server.marks_placed(leaves, leaves) {
                    server.placed.insert(pos);
                }
                server.reward_break(1, pos, None)
            })
            .count();
        assert_eq!(sticks, 0, "placed leaves gave a stick");
        assert!(!server.placed.contains(&pos), "the mark goes with the break");
    }
}

#[cfg(test)]
mod finished_tests {
    use std::sync::Arc;
    use std::time::Instant;

    use voxmar_engine::action::ActionTarget;
    use voxmar_engine::coords::{ColumnPos, Face};
    use voxmar_engine::glam::{IVec3, Vec3};
    use voxmar_engine::inventory::ItemStack;
    use voxmar_engine::world::Column;

    const FLOOR: i32 = 199;

    /// A stone floor, and player 1 on it with a stone pickaxe in the first slot.
    fn fixture() -> crate::Server {
        let mut server = crate::test_server();
        server.world = Default::default();
        let stone = server.content.blocks.expect("base:stone");
        let mut column = Column::default();
        for x in 0..32 {
            for z in 0..32 {
                column.set_block(IVec3::new(x, FLOOR, z), stone);
            }
        }
        server.world.insert(ColumnPos::new(0, 0), Arc::new(column));
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let at = Vec3::new(7.5, (FLOOR + 1) as f32, 8.5);
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        let pick = server.content.blocks.expect("base:stone_pickaxe");
        player.inventory.slots[0] = Some(ItemStack::new(pick, 1));
        player.selected = 0;
        server.players.insert(1, player);
        server
    }

    /// A block the client shows broken before most of its time is up (its proposal reached
    /// the server late) still breaks when its time comes, whatever the player takes in hand
    /// next: the next block's tool, or the bare hand once the pickaxe is put away. Stone
    /// refuses the bare hand, so judged by what is held at its time, it came back.
    #[test]
    fn a_break_shown_done_early_stays_broken_whatever_is_held_next() {
        let take_up: [(&str, fn(&mut crate::player::Player)); 2] = [
            ("the next slot's bare hand", |p| p.selected = 1),
            ("the pickaxe put away", |p| p.inventory.slots[0] = None),
        ];
        for (next, take_up) in take_up {
            let mut server = fixture();
            let cell = IVec3::new(10, FLOOR, 8);
            let target = ActionTarget::Block { block: cell, face: Face::PosY };
            let mine = server.content.actions.expect("core:mine");
            let at = Instant::now();
            server.propose(1, mine, target, at);
            let due = server.executor.timed.iter().find(|t| t.target == target).expect("a timed break").due;
            server.finished_action(1, target, at + (due - at).mul_f32(0.3));
            assert!(server.executor.timed.iter().any(|t| t.target == target && t.finished), "shown done early, it waits for its time");
            take_up(server.players.get_mut(&1).unwrap());
            server.complete_actions(due);
            let now = server.block(cell).map(|b| server.content.blocks.def(b).name.clone());
            assert_eq!(now.as_deref(), Some("core:air"), "the stone came back with {next}");
        }
    }

    /// What the player held when the client showed it done is what the break is judged by,
    /// not a pass: begun with the pickaxe but shown done by hand, stone still refuses it,
    /// though the pickaxe is back in hand at its time.
    #[test]
    fn a_break_shown_done_is_judged_by_what_was_held_then() {
        let mut server = fixture();
        let cell = IVec3::new(10, FLOOR, 8);
        let target = ActionTarget::Block { block: cell, face: Face::PosY };
        let mine = server.content.actions.expect("core:mine");
        let at = Instant::now();
        server.propose(1, mine, target, at);
        let due = server.executor.timed.iter().find(|t| t.target == target).expect("a timed break").due;
        server.players.get_mut(&1).unwrap().selected = 1;
        server.finished_action(1, target, at + (due - at).mul_f32(0.3));
        server.players.get_mut(&1).unwrap().selected = 0;
        server.complete_actions(due);
        let now = server.block(cell).map(|b| server.content.blocks.def(b).name.clone());
        assert_eq!(now.as_deref(), Some("base:stone"), "stone broke by hand once the pickaxe was taken up");
    }

    /// The world is judged again all the same: a block gone by its time (someone else broke
    /// it) is not broken twice, and gives nothing.
    #[test]
    fn a_break_shown_done_whose_block_went_meanwhile_is_refused() {
        let mut server = fixture();
        let cell = IVec3::new(10, FLOOR, 8);
        let target = ActionTarget::Block { block: cell, face: Face::PosY };
        let mine = server.content.actions.expect("core:mine");
        let at = Instant::now();
        server.propose(1, mine, target, at);
        let due = server.executor.timed.iter().find(|t| t.target == target).expect("a timed break").due;
        server.finished_action(1, target, at + (due - at).mul_f32(0.3));
        server.set_block(cell, voxmar_engine::block::BlockId::AIR);
        server.complete_actions(due);
        assert!(server.executor.timed.is_empty(), "it was judged at its time");
        let had = server.players[&1].inventory.slots.iter().flatten().filter(|s| server.content.blocks.def(s.item).name != "base:stone_pickaxe").count();
        assert_eq!(had, 0, "a break of a block already gone gave something");
        assert_eq!(server.items.saved(|_| String::new()).len(), 0, "a break of a block already gone dropped something");
    }

    /// Only a break keeps what was held: a bite shown done early and then swapped for
    /// something that is not food is judged by what is in hand at its time, and refused,
    /// eating neither.
    #[test]
    fn a_bite_shown_done_early_is_judged_by_what_is_held_at_its_time() {
        let mut server = fixture();
        let (stew, stone) = (server.content.blocks.expect("base:stew"), server.content.blocks.expect("base:stone"));
        let player = server.players.get_mut(&1).unwrap();
        player.inventory.slots[0] = Some(ItemStack::new(stew, 1));
        player.inventory.slots[1] = Some(ItemStack::new(stone, 1));
        // Hurt, or a bite is refused.
        player.health.current = 1;
        let eat = server.content.actions.expect("core:eat");
        let at = Instant::now();
        server.propose(1, eat, ActionTarget::None, at);
        let due = server.executor.timed.iter().find(|t| t.target == ActionTarget::None).expect("a timed bite").due;
        server.finished_action(1, ActionTarget::None, at + (due - at).mul_f32(0.3));
        server.players.get_mut(&1).unwrap().selected = 1;
        server.complete_actions(due);
        let slots = &server.players[&1].inventory.slots;
        assert_eq!((slots[0], slots[1]), (Some(ItemStack::new(stew, 1)), Some(ItemStack::new(stone, 1))), "a bite swapped for stone ate something");
        assert_eq!(server.players[&1].health.current, 1, "a bite swapped for stone healed");
    }

    /// Breaks shown done early run one after another: the next one's time starts when the
    /// one before is due, so five stones shown done at three tenths of their time each take
    /// as long as five broken honestly.
    #[test]
    fn breaks_shown_done_early_never_run_side_by_side() {
        let mut server = fixture();
        let mine = server.content.actions.expect("core:mine");
        let start = Instant::now();
        let mut at = start;
        let mut dues = Vec::new();
        for x in 0..5 {
            let target = ActionTarget::Block { block: IVec3::new(5 + x, FLOOR, 10), face: Face::PosY };
            server.propose(1, mine, target, at);
            let timed = server.executor.timed.iter().find(|t| t.target == target).expect("a timed break");
            let (started, due) = (timed.started, timed.due);
            dues.push(due);
            at += (due - started).mul_f32(0.3);
            server.finished_action(1, target, at);
        }
        let each = dues[0] - start;
        for (i, due) in dues.iter().enumerate() {
            assert_eq!(*due - start, each * (i as u32 + 1), "break {i} is due after the one before");
        }
        server.complete_actions(dues[1]);
        let broken = (0..5).filter(|x| server.block(IVec3::new(5 + x, FLOOR, 10)) == Some(voxmar_engine::block::BlockId::AIR)).count();
        assert_eq!(broken, 2, "by the second's time, two are broken");
        server.complete_actions(dues[4]);
        assert!((0..5).all(|x| server.block(IVec3::new(5 + x, FLOOR, 10)) == Some(voxmar_engine::block::BlockId::AIR)), "all five in their time");
    }

    /// A break shown done early wears the pickaxe in hand then, not the shovel switched to
    /// before its time, nor another copy of it the break was begun with. Shown done with
    /// something else (another pickaxe, the bare hand) the tool that set its time wears, so
    /// a switch at the last moment never dodges the wear.
    #[test]
    fn a_break_shown_done_early_wears_the_tool_it_was_shown_done_with() {
        let mut server = fixture();
        let (pick, shovel) = (server.content.blocks.expect("base:stone_pickaxe"), server.content.blocks.expect("base:stone_shovel"));
        let flint = server.content.blocks.expect("base:flint_pickaxe");
        let player = server.players.get_mut(&1).unwrap();
        player.inventory.slots[1] = Some(ItemStack::new(shovel, 1));
        player.inventory.slots[2] = Some(ItemStack::new(pick, 1));
        player.inventory.slots[3] = Some(ItemStack::new(flint, 1));
        let mine = server.content.actions.expect("core:mine");
        let wear = |server: &crate::Server| server.players[&1].inventory.slots[..4].iter().map(|s| s.map(|s| s.wear)).collect::<Vec<_>>();
        // Five breaks begun with `began` in hand, shown done at three tenths of their time
        // with `shown`, and the shovel in hand from there to their time. Five blocks: skill
        // cannot save every use.
        let breaks = |server: &mut crate::Server, began: usize, shown: usize, row: i32| {
            for x in 0..5 {
                let cell = IVec3::new(5 + x, FLOOR, row);
                let target = ActionTarget::Block { block: cell, face: Face::PosY };
                server.players.get_mut(&1).unwrap().selected = began;
                let at = Instant::now();
                server.propose(1, mine, target, at);
                let due = server.executor.timed.iter().find(|t| t.target == target).expect("a timed break").due;
                server.players.get_mut(&1).unwrap().selected = shown;
                server.finished_action(1, target, at + (due - at).mul_f32(0.3));
                server.players.get_mut(&1).unwrap().selected = 1;
                server.complete_actions(due);
                let now = server.block(cell).map(|b| server.content.blocks.def(b).name.clone());
                assert_eq!(now.as_deref(), Some("core:air"), "the block at {cell} came back");
            }
        };
        breaks(&mut server, 0, 0, 10);
        let after = wear(&server);
        assert!(after[0].is_some_and(|w| w > 0) && after[1..] == [Some(0), Some(0), Some(0)], "the pickaxe wore, not the shovel switched to: {after:?}");
        breaks(&mut server, 2, 0, 6);
        let last = wear(&server);
        assert!(last[0] > after[0] && last[1..] == [Some(0), Some(0), Some(0)], "begun with the other pickaxe, the one shown done with wore: {after:?} to {last:?}");
        breaks(&mut server, 0, 3, 11);
        let flinted = wear(&server);
        assert!(flinted[0] > last[0] && flinted[1..] == [Some(0), Some(0), Some(0)], "shown done with the flint pickaxe, the stone one that set the time wore: {last:?} to {flinted:?}");
        let dirt = server.content.blocks.expect("base:dirt");
        for x in 0..5 {
            server.set_block(IVec3::new(5 + x, FLOOR, 5), dirt);
        }
        breaks(&mut server, 0, 4, 5);
        let bare = wear(&server);
        assert!(bare[0] > flinted[0] && bare[1..] == [Some(0), Some(0), Some(0)], "shown done with a bare hand, the pickaxe wore: {flinted:?} to {bare:?}");
    }
}
