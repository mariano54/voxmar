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

use voxmar_engine::action::{ActionContext, ActionDef, ActionId, ActionTarget, Effect, placed_block, placement_cell, supported_by};
use voxmar_engine::block::{BlockId, BlockInfo, BlockRegistry};
use voxmar_engine::crop::CropRegistry;
use voxmar_engine::skill::SkillId;
use voxmar_engine::glam::{IVec3, Vec3};
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

impl Server {
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
    Context { server, agent }
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
        self.server.players[&self.agent].held()
    }

    fn actor_pouched(&self) -> Vec<BlockId> {
        self.server.players[&self.agent].inventory.pouched.iter().map(|&(item, _)| item).collect()
    }

    fn actor_hurt(&self) -> bool {
        let health = self.server.players[&self.agent].health;
        health.current < health.max
    }

    fn actor_mining_speed(&self) -> f32 {
        self.server.players[&self.agent].modifiers.mining
    }

    fn occupied(&self, cell: IVec3) -> bool {
        self.server.players.values().any(|p| p.joined && p.health.alive() && p.aabb().intersects(|c| c == cell))
    }

    fn creature_in(&self, cell: IVec3) -> bool {
        self.server.creatures.live.values().any(|c| c.health > 0 && c.body.aabb().intersects(|at| at == cell))
    }

    fn cooking(&self) -> Option<&voxmar_engine::cooking::CookingRegistry> {
        Some(&self.server.content.cooking)
    }

    fn tools(&self) -> Option<&voxmar_engine::tool::ToolRegistry> {
        Some(&self.server.content.tools)
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
        let context = Context { server: self, agent };
        let seconds = def.seconds(&context, target);
        let timed = seconds >= INSTANT;
        if timed {
            self.supersede_action(agent, at);
        }
        let context = Context { server: self, agent };
        if let Err(why) = def.validate(&context, target) {
            tracing::debug!("player {agent}: {} refused at {target:?}: {why:?}", def.name);
            self.reject(agent, target);
        } else if !timed {
            self.commit(agent, &def, target);
        } else {
            if def.effects.contains(&Effect::EatHeld) {
                self.bite_started(agent);
            }
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

    /// Applies an action's effects, as `agent`'s doing (`fire_origin`, `changed_by`) whenever
    /// it commits: as their message arrives or later, when it falls due in the run loop.
    fn commit(&mut self, agent: PlayerId, def: &ActionDef, target: ActionTarget) {
        let by = self.players.get(&agent).map(|p| crate::player_key(&p.name));
        let fire_was = std::mem::replace(&mut self.fire_origin, by.clone());
        let changed_was = std::mem::replace(&mut self.changed_by, by);
        self.commit_effects(agent, def, target);
        self.fire_origin = fire_was;
        self.changed_by = changed_was;
    }

    /// Applies an action's effects. Validation already guaranteed they can all apply.
    fn commit_effects(&mut self, agent: PlayerId, def: &ActionDef, target: ActionTarget) {
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
                    let plants = supported_by(&Context { server: self, agent }, block);
                    let placed: Vec<IVec3> = std::iter::once(block).chain(plants.iter().copied()).filter(|p| self.placed.contains(p)).collect();
                    if let Some(broken) = self.block(block).filter(|_| !placed.contains(&block)) {
                        self.discover_broken(agent, broken);
                    }
                    self.picked(agent, block);
                    let instead = self.reward_break(agent, block);
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
                    let placed = placed_block(&Context { server: self, agent }, item, block, face);
                    self.set_block(cell, placed);
                    // Its upper half goes in above with it (a door's top).
                    if let Some(upper) = self.content.blocks.upper(placed) {
                        self.set_block(cell + IVec3::Y, upper);
                    }
                    // Marked as the player's: an ore teaches nothing broken again, and a
                    // furnishing gives itself back (where the world placed one, it does not).
                    // (By the block placed: a held item may place as another, as a torch does.)
                    if self.content.tools.rule(item).is_some_and(|rule| rule.xp > 0.0) || self.content.blocks.def(placed).has_tag("furnishing") {
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
                    if player.inventory.move_items(from as usize, to as usize, count) {
                        player.changed(traits::INVENTORY);
                        self.push_store(agent);
                    }
                }
                (Effect::QuickMove, ActionTarget::Slot(slot)) => {
                    self.pull_store(agent);
                    let player = self.players.get_mut(&agent).unwrap();
                    if player.inventory.quick_move(slot as usize) {
                        player.changed(traits::INVENTORY);
                        self.push_store(agent);
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
                }
                (Effect::Craft, ActionTarget::Slot(times)) => self.craft(agent, times == 0),
                (Effect::LayRecipe, ActionTarget::Recipe { first, ways, count }) => self.lay_recipe(agent, first as usize, ways as usize, count),
                (Effect::Fish, ActionTarget::Block { block, .. }) => self.fish(agent, block),
                (Effect::Swap, ActionTarget::Block { block, .. }) => {
                    // Opened or shut here first: the clients follow what the server holds.
                    let changes = voxmar_engine::action::swapping(&Context { server: self, agent }, block);
                    self.set_blocks(&changes);
                }
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
    /// bed allows (effects.ron's `sleeps`). A bed always works, as a bed; the rest is held back
    /// while one is still on them, so the buff cannot be had again until it wears off.
    fn sleep(&mut self, agent: PlayerId, bed: IVec3) {
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
        if effects.rested(&self.players[&agent].timed_effects) {
            let player = &self.players[&agent];
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("bed.not_tired") });
            return;
        }
        let home = self.home_survey(agent);
        let feet = self.players[&agent].position.floor().as_ivec3();
        let outdoors = self.open_sky(feet + IVec3::Y * 2);
        let calm = voxmar_engine::weather::Weather::NONE;
        let situation = voxmar_engine::effect::Situation { felt: 0.0, warmed: false, outdoors, weather: &calm, biome: "", biome_tags: &[], home: home.as_ref(), done: &self.players[&agent].quests_done, held: &[], near: &[], seated: false };
        // A proper bed rests better than a bag on the ground (effects.ron's `bed` tags).
        let bed_tags = self.block(bed).map(|b| self.content.blocks.def(b).tags.clone()).unwrap_or_default();
        let given = effects.given_by_sleep(&situation, &bed_tags);
        // The best rest it gives (the one the others are hidden by) is the one it says.
        let best = given.iter().map(|&(id, _)| id).filter(|&id| !given.iter().any(|&(other, _)| other != id && effects.hides(other, id))).next_back();
        let player = self.players.get_mut(&agent).unwrap();
        player.give_effects(&given);
        player.changed(traits::VITALS);
        if let Some(best) = best {
            let def = effects.get(best);
            let key = voxmar_engine::lang::def_key("effect", &def.name);
            let words = Words::key("bed.rested").with("name", Words::Own(key.clone(), def.title.clone())).with("what", Words::Own(format!("{key}.description"), def.description.clone()));
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
        }
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

    /// What breaking the block at `pos` teaches (`tools.ron`), and sometimes an extra drop:
    /// more often with the skill's level above what the block asks and a better tool. A
    /// block a player placed teaches nothing and gives nothing extra.
    /// Returns whether it gave something else instead of the block's own drop.
    fn reward_break(&mut self, agent: PlayerId, pos: IVec3) -> bool {
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
        let held = player.held();
        let level = rule.skill.map_or(1, |skill| self.content.skills.level(player.skills.xp(skill)));
        let bonus = voxmar_engine::crop::level_bonus(level, rule.level, self.content.skills.max_level()) + tools.yield_bonus(held, block) + player.modifiers.finds;
        let info = self.content.blocks.info(block);
        // The pops show where each drop flies, so they draw nothing from the player's rolls.
        let [a, b, c] = self.meta.next_pop(pos, agent ^ 0xd1ce);
        // Never a furnishing's: it gives itself back only where a player placed it.
        if roll < bonus && !info.drops.is_air() && info.drop_count > 0 && !self.content.blocks.def(block).has_tag("furnishing") {
            let stack = voxmar_engine::inventory::ItemStack { item: info.drops, count: 1 };
            let body = voxmar_engine::dropped::popped(pos, [a, b, c]);
            self.items.spawn(stack, body, voxmar_engine::dropped::PICKUP_DELAY);
        }
        if let Some(skill) = rule.skill.filter(|_| rule.xp > 0.0) {
            self.grant_xp(agent, skill, rule.xp);
        }
        match rule.instead {
            Some((item, chance)) if other < chance => {
                let body = voxmar_engine::dropped::popped(pos, [b, c, a]);
                self.items.spawn(voxmar_engine::inventory::ItemStack { item, count: 1 }, body, voxmar_engine::dropped::PICKUP_DELAY);
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
        let tags = self.content.recipes.near_tags();
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
                    server.reward_break(1, pos)
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
}
