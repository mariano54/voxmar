//! Players: agents whose controller is a remote input device. A player's session handles
//! the handshake and validates movement; everything else they do goes through actions.

use std::time::Instant;

use rustc_hash::FxHashSet;
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::effect::{EffectId, EffectRegistry, Modifiers};
use voxmar_engine::glam::Vec3;
use voxmar_engine::inventory::{HOTBAR_SLOTS, Inventory, ItemStack, MAX_STACK};
use voxmar_engine::lang::Words;
use voxmar_engine::physics::{Aabb, player as dims};
use voxmar_engine::skill::{SkillRegistry, Skills};
use voxmar_engine::traits::{self, Health, TraitDecl};
use voxmar_engine::vitals::Vitals;
use voxmar_protocol::{
    ChatKind, ClientMessage, MAX_FLY_SPEED, PROTOCOL_VERSION, PlayerId, PlayerInput, PlayerView, ServerConnection, ServerMessage,
    TICKS_PER_SECOND,
};

use crate::Server;
use crate::save::{GameMode, SavedPlayer};

/// How much faster than the content's top speed a player's movement may be, for rounding,
/// currents and network jitter.
const SPEED_MARGIN: f32 = 1.5;
/// Seconds of top speed a player's movement may run ahead in one go: a client that
/// hitched sends one long step, and inputs bunch up on their way.
const MOVE_BURST: f32 = 1.0;
/// Blocks of movement allowed besides.
const MOVE_SLACK: f32 = 0.5;
/// How far a player on foot may rise above where they last stood or swam, besides the
/// height of their jump: the lift onto a bank out of water, and slack.
const RISE_SLACK: f32 = voxmar_engine::movement::STEP_OUT + 0.25;
/// Longest step a movement is checked for walls in, in blocks.
const SWEEP_STEP: f32 = 0.25;
/// Messages a player's handled per tick, at most; the rest wait for the next (and a
/// connection whose queue fills is cut off). Inputs come one a tick.
pub const MESSAGES_PER_TICK: u32 = 64;
/// Lines of chat in a burst, and per second after it.
const CHAT_BURST: f32 = 8.0;
const CHAT_PER_SECOND: f32 = 2.0;
/// Ticks a connection may take to say hello before it is closed.
pub const HELLO_TICKS: u64 = 10 * TICKS_PER_SECOND as u64;

pub struct Player {
    pub id: PlayerId,
    pub name: String,
    pub connection: ServerConnection,
    pub joined: bool,
    pub admin: bool,
    /// Creative mode: the player may fly.
    pub allow_flight: bool,
    /// Radius, in columns, streamed around the player.
    pub view_distance: i32,
    /// Flight speed multiplier; above 1 only for admins.
    pub fly_speed: f32,
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub selected: usize,
    /// The crafting table the player last opened, whose three-by-three grid they may use
    /// while within reach of it.
    pub station: Option<voxmar_engine::glam::IVec3>,
    pub flying: bool,
    pub health: Health,
    pub inventory: Inventory,
    pub skills: Skills,
    /// Hunger, warmth and their effects.
    pub vitals: Vitals,
    /// Effects food gave, with seconds left, and every effect's modifiers together, as of
    /// the last second (`process::Survival`).
    pub timed_effects: Vec<(EffectId, f32)>,
    pub modifiers: Modifiers,
    /// Healing and hurting under way, in seconds (`process::Survival`).
    pub heal_clock: f32,
    pub hurt_clock: f32,
    /// What the player ate lately, for how much the same food heals again, and the
    /// seconds toward its next recovery.
    pub recent_food: voxmar_engine::vitals::Recent,
    /// Where the player wakes after dying (a bed or hearth sets it), and whether what
    /// they carried has been left where they fell since they last died (`death`).
    pub home: Option<Vec3>,
    /// The bed (a sleeping bag) last slept in, where they wake without a home.
    pub bed: Option<Vec3>,
    pub buried: bool,
    /// Everything the player has discovered (`journal`).
    pub journal: voxmar_engine::journal::Journal,
    /// Quests finished, tallies of what quests count (`crafted:base:torch`), and the quest
    /// states last sent (`quests`).
    pub quests_done: Vec<String>,
    pub tallies: rustc_hash::FxHashMap<String, u32>,
    pub quests_sent: Vec<voxmar_engine::quest::QuestState>,
    pub food_clock: f32,
    /// Highest point of the current fall, if airborne.
    pub fall_start: Option<f32>,
    /// Ground the player may still cover, in blocks, and when it was last topped up
    /// (`Server::input`): it refills at their top speed by the clock their messages arrive
    /// on, so a burst of inputs gets no more ground than the time they span.
    pub move_budget: f32,
    pub budget_at: Option<Instant>,
    /// Where the player's own input last put them (the server moving them, by a teleport
    /// or a respawn, starts afresh), and the height they last stood, swam or flew at, which
    /// on foot they may rise above by a jump only.
    pub accepted: Vec3,
    pub support_y: f32,
    /// The tick the player connected, for how long they may take to say hello.
    pub connected_tick: u64,
    /// Messages the player may still send this tick, and lines of chat (and, for those not
    /// admins, commands) they may still send before having to slow down.
    pub messages_left: u32,
    pub chat_allowance: f32,
    /// Owner-replicated traits changed since they were last sent.
    pub dirty: Vec<TraitDecl>,
    pub sent: FxHashSet<ColumnPos>,
    /// Dropped items this player has been told about.
    pub items_seen: FxHashSet<u32>,
}

impl Player {
    pub fn new(id: PlayerId, connection: ServerConnection, spawn: Vec3, view_distance: i32, tick: u64, max_health: u8) -> Self {
        Self {
            id,
            name: String::new(),
            connection,
            joined: false,
            admin: false,
            allow_flight: false,
            view_distance,
            fly_speed: 1.0,
            position: spawn,
            yaw: 0.0,
            pitch: 0.0,
            selected: 0,
            station: None,
            flying: false,
            health: Health::full(max_health),
            inventory: Inventory::default(),
            skills: Skills::default(),
            vitals: Vitals::default(),
            timed_effects: Vec::new(),
            modifiers: Modifiers::default(),
            recent_food: Default::default(),
            home: None,
            bed: None,
            buried: false,
            journal: Default::default(),
            quests_done: Vec::new(),
            tallies: Default::default(),
            quests_sent: Vec::new(),
            food_clock: 0.0,
            heal_clock: 0.0,
            hurt_clock: 0.0,
            fall_start: None,
            move_budget: 0.0,
            budget_at: None,
            accepted: spawn,
            support_y: spawn.y,
            connected_tick: tick,
            messages_left: MESSAGES_PER_TICK,
            chat_allowance: CHAT_BURST,
            dirty: vec![traits::HEALTH, traits::INVENTORY, traits::SKILLS, traits::VITALS],
            sent: FxHashSet::default(),
            items_seen: FxHashSet::default(),
        }
    }

    pub fn view(&self) -> PlayerView {
        PlayerView { id: self.id, name: self.name.clone(), position: self.position, yaw: self.yaw }
    }

    pub fn aabb(&self) -> Aabb {
        Aabb::standing(self.position, dims::WIDTH, dims::HEIGHT)
    }

    pub fn eye(&self) -> Vec3 {
        self.position + Vec3::Y * dims::EYE_HEIGHT
    }

    /// Where the player looks.
    pub fn look(&self) -> Vec3 {
        let (yaw, pitch) = (self.yaw, self.pitch);
        Vec3::new(-yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos())
    }

    pub fn held(&self) -> Option<BlockId> {
        self.inventory.get(self.selected).map(|stack| stack.item)
    }

    pub fn changed(&mut self, decl: TraitDecl) {
        if !self.dirty.iter().any(|d| d.name == decl.name) {
            self.dirty.push(decl);
        }
    }

    pub fn mode(&self) -> GameMode {
        if self.allow_flight { GameMode::Creative } else { GameMode::Survival }
    }

    /// Gives timed effects (eaten food's), topping up any it has to the longer time.
    pub fn give_effects(&mut self, given: &[(EffectId, f32)]) {
        for &(effect, seconds) in given {
            match self.timed_effects.iter_mut().find(|(e, _)| *e == effect) {
                Some((_, left)) => *left = left.max(seconds),
                None => self.timed_effects.push((effect, seconds)),
            }
        }
    }

    /// The persistent traits, for saving.
    pub fn saved(&self, skills: &SkillRegistry, effects: &EffectRegistry) -> SavedPlayer {
        SavedPlayer {
            position: self.position,
            yaw: self.yaw,
            mode: Some(self.mode()),
            flying: self.flying,
            health: self.health,
            inventory: self.inventory.clone(),
            skills: skills.ids().map(|id| (skills.get(id).name.clone(), self.skills.xp(id))).filter(|(_, xp)| *xp > 0).collect(),
            effects: self.timed_effects.iter().map(|&(id, left)| (effects.get(id).name.clone(), left)).collect(),
            home: self.home.map(|h| h.to_array()),
            bed: self.bed.map(|b| b.to_array()),
            journal: self.journal.entries().cloned().collect(),
            quests: self.quests_done.clone(),
            tallies: { let mut t: Vec<(String, u32)> = self.tallies.iter().map(|(k, &v)| (k.clone(), v)).collect(); t.sort(); t },
        }
    }
}

impl Server {
    /// Handles a message that arrived `at`. The player's timed action due by then
    /// completes first: the player saw it done before sending this.
    pub(crate) fn handle_message(&mut self, id: PlayerId, message: ClientMessage, at: Instant) {
        self.arrival = at;
        if self.players[&id].joined {
            self.complete_actions_of(id, at);
        }
        match message {
            ClientMessage::Hello { name, protocol, content } => self.hello(id, name, protocol, content),
            _ if !self.players[&id].joined => {}
            ClientMessage::Input(input) => self.input(id, input),
            ClientMessage::Propose { action, target } => self.propose(id, action, target, at),
            ClientMessage::Cancel => self.cancel_action(id),
            ClientMessage::Finished { target } => self.finished_action(id, target, at),
            ClientMessage::Settings { view_distance, fly_speed } => self.settings(id, view_distance, fly_speed),
            ClientMessage::Command { line } => {
                if self.may_speak(id, true) {
                    self.command(id, &line);
                }
            }
            ClientMessage::Chat { text } => {
                if self.may_speak(id, false) {
                    self.chat(id, &text);
                }
            }
            ClientMessage::Take { item, count, held } => self.take_from_catalogue(id, item, count, held),
        }
    }

    /// Creative mode's catalogue: `count` of any item onto the pointer (topping up a stack
    /// of it there) or into the inventory, where what does not fit is not given. A survival
    /// player gets nothing.
    fn take_from_catalogue(&mut self, id: PlayerId, item: BlockId, count: u8, held: bool) {
        let known = (item.0 as usize) < self.content.blocks.len() && !item.is_air();
        let Some(player) = self.players.get_mut(&id) else { return };
        if !known || count == 0 || player.mode() != GameMode::Creative {
            return;
        }
        let count = count.min(MAX_STACK);
        if held {
            match &mut player.inventory.cursor {
                None => player.inventory.cursor = Some(ItemStack { item, count }),
                Some(stack) if stack.item == item => stack.count = stack.count.saturating_add(count).min(MAX_STACK),
                Some(_) => return,
            }
        } else {
            player.inventory.add(item, count);
        }
        player.changed(traits::INVENTORY);
    }

    fn hello(&mut self, id: PlayerId, name: String, protocol: u32, content: u64) {
        let fingerprint = self.content.fingerprint();
        let blocks = &self.content.blocks;
        let start_kit = self.survival.kit(blocks);
        let (seed, allow_flight) = (self.meta.seed, self.config.allow_flight);
        let name: String = name.chars().filter(|c| !c.is_control()).take(32).collect();
        let name = if name.trim().is_empty() { "player".to_owned() } else { name };
        if self.players[&id].joined {
            return;
        }
        // One player per name: a second login would load the same save twice, so whatever
        // either carried would be doubled and their saves would overwrite each other.
        let reason = if protocol != PROTOCOL_VERSION {
            Some(Words::key("net.rejected.protocol").with("theirs", Words::text(protocol)).with("ours", Words::text(PROTOCOL_VERSION)))
        } else if content != fingerprint {
            Some(Words::key("net.rejected.content"))
        } else if self.players.values().any(|p| p.joined && p.name == name) {
            Some(Words::key("net.rejected.name_taken").with("name", name.as_str()))
        } else {
            None
        };
        if let Some(reason) = reason {
            self.players[&id].connection.send(ServerMessage::Rejected { reason });
            return;
        }
        // An unreadable save is moved aside rather than replaced by a new player's at the
        // next save; if it cannot even be moved, the player is turned away.
        let (saved, set_aside) = match self.save.as_ref().map(|save| (save, save.read_player(&name, blocks))) {
            None => (None, None),
            Some((_, Ok(saved))) => (saved, None),
            Some((save, Err(e))) => match save.set_aside_player(&name) {
                Ok(aside) => {
                    tracing::error!("could not load player {name}: {e}; kept it as {} and started them afresh", aside.display());
                    (None, aside.file_name().map(|n| n.to_string_lossy().into_owned()))
                }
                Err(moving) => {
                    tracing::error!("could not load player {name}: {e}; nor move it aside: {moving}");
                    self.players[&id].connection.send(ServerMessage::Rejected { reason: Words::key("net.rejected.save") });
                    return;
                }
            },
        };
        let local_admin = self.config.local_admin;
        let skill_ids = self.content.skills.clone();
        let effect_ids = self.content.effects.clone();
        let weather_seed = self.weather.seed();
        let player = self.players.get_mut(&id).expect("message from unknown player");

        player.name = name;
        player.joined = true;
        // Only the integrated server's own player can connect in-process. Names are not
        // authenticated, so a name on the admin list counts only from this machine; an admin
        // makes others admins with `/op`.
        player.admin = (player.connection.is_local() && local_admin)
            || (player.connection.is_loopback() && self.config.admins.contains(&player.name));
        player.allow_flight = saved.as_ref().and_then(|saved| saved.mode).map_or(allow_flight, GameMode::allows_flight);
        // Returning players carry on where they left; new ones start at spawn with a kit.
        match saved {
            Some(saved) if saved.health.alive() => {
                player.position = saved.position;
                player.yaw = saved.yaw;
                player.flying = saved.flying && player.allow_flight;
                player.health = saved.health;
                player.inventory = saved.inventory;
                player.skills = restore_skills(&saved.skills, &skill_ids);
                player.home = saved.home.map(Vec3::from_array);
                player.bed = saved.bed.map(Vec3::from_array);
                player.journal = voxmar_engine::journal::Journal::from_entries(saved.journal);
                player.quests_done = saved.quests;
                player.tallies = saved.tallies.into_iter().collect();
                player.timed_effects = saved.effects.iter().filter_map(|(name, left)| Some((effect_ids.id(name)?, *left))).collect();
            }
            // Players who left dead come back to life at the spawn point.
            Some(saved) => {
                player.inventory = saved.inventory;
                player.skills = restore_skills(&saved.skills, &skill_ids);
                player.journal = voxmar_engine::journal::Journal::from_entries(saved.journal);
                player.quests_done = saved.quests;
                player.tallies = saved.tallies.into_iter().collect();
            }
            None => {
                for (block, count) in start_kit {
                    player.inventory.add(block, count);
                }
            }
        }
        skill_ids.start(&mut player.skills);
        player.changed(traits::INVENTORY);
        player.changed(traits::HEALTH);
        player.changed(traits::SKILLS);
        player.changed(traits::VITALS);
        let admin = player.admin;
        player.connection.send(ServerMessage::Welcome {
            player: id,
            seed,
            weather_seed,
            position: player.position,
            yaw: player.yaw,
            flying: player.flying,
            allow_flight: player.allow_flight,
            admin,
        });
        player.budget_at = None;
        tracing::info!("{} joined", player.name);
        let time = self.time_message();
        self.players[&id].connection.send(time);
        let entries = self.players[&id].journal.entries().cloned().collect();
        self.players[&id].connection.send(ServerMessage::Journal { entries });
        if let Some(file) = set_aside {
            let text = Words::key("save.player_set_aside").with("file", Words::text(file));
            self.players[&id].connection.send(ServerMessage::Chat { kind: ChatKind::Error, text });
        }
    }

    /// Gives every player their share of messages for the tick and tops up their chat,
    /// and closes connections that have not said hello in time.
    pub(crate) fn limit_players(&mut self) {
        let tick = self.tick;
        self.players.retain(|_, player| {
            player.messages_left = MESSAGES_PER_TICK;
            player.chat_allowance = (player.chat_allowance + CHAT_PER_SECOND / TICKS_PER_SECOND as f32).min(CHAT_BURST);
            player.joined || tick.saturating_sub(player.connected_tick) <= HELLO_TICKS
        });
    }

    /// Whether the player may send a line of chat (or a command, unless an admin) now;
    /// if not, they are told to slow down.
    pub(crate) fn may_speak(&mut self, id: PlayerId, command: bool) -> bool {
        let Some(player) = self.players.get_mut(&id) else { return false };
        if command && player.admin {
            return true;
        }
        if player.chat_allowance < 1.0 {
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Error, text: Words::key("chat.too_fast") });
            return false;
        }
        player.chat_allowance -= 1.0;
        true
    }

    /// Accepts the client's predicted movement if it is physically plausible, otherwise
    /// snaps the client back. Plausible is within the ground their budget allows (no
    /// faster than their top speed by the clock, however many inputs they send), on foot
    /// no higher above where they last stood or swam than a jump takes them, no faster
    /// down than a fall, and through no wall on the way.
    fn input(&mut self, id: PlayerId, input: PlayerInput) {
        let at = self.arrival;
        let player = &self.players[&id];
        if !player.health.alive() || !input.position.is_finite() || !input.yaw.is_finite() || !input.pitch.is_finite() {
            return;
        }
        // Quicker on foot as their effects say, as the client moves them.
        let tuning = &self.content.movement.on_foot(player.modifiers.speed.max(1.0));
        let flying = input.flying && player.allow_flight;
        let speed = SPEED_MARGIN
            * if flying { tuning.fly_sprint_speed * player.fly_speed } else { tuning.top_ground_speed().max(tuning.jump_velocity) };
        let cap = speed * MOVE_BURST + MOVE_SLACK;
        let since = player.budget_at.map_or(1.0 / TICKS_PER_SECOND as f32, |then| at.saturating_duration_since(then).as_secs_f32()).min(MOVE_BURST);
        let from = player.position;
        // Moved by the server since their last input (a teleport, a respawn): afresh.
        let (budget, support_y) = if from != player.accepted || player.budget_at.is_none() {
            (cap, from.y)
        } else {
            let supported = (player.flying && player.allow_flight) || self.supported(from);
            ((player.move_budget + since * speed).min(cap), if supported { from.y } else { player.support_y })
        };
        let delta = input.position - from;
        let cost = if flying { delta.length() } else { delta.with_y(0.0).length() + delta.y.max(0.0) };
        let jump = tuning.jump_velocity * tuning.jump_velocity / (2.0 * tuning.gravity);
        let too_high = !flying && input.position.y > support_y + jump + RISE_SLACK;
        let too_fast_down = -delta.y > tuning.terminal_velocity * SPEED_MARGIN * since + 1.5;
        let plausible = cost <= budget && !too_high && !too_fast_down && !self.blocked_between(from, input.position);

        let player = self.players.get_mut(&id).unwrap();
        player.yaw = input.yaw;
        player.pitch = input.pitch.clamp(-1.6, 1.6);
        player.flying = flying;
        player.selected = (input.selected_slot as usize).min(HOTBAR_SLOTS - 1);
        player.budget_at = Some(at);
        player.support_y = support_y;
        if plausible {
            player.move_budget = budget - cost;
            player.position = input.position;
            player.accepted = input.position;
        } else {
            player.move_budget = budget;
            player.connection.send(ServerMessage::Teleport { position: player.position });
        }
    }

    /// Whether a player standing at `feet` has ground under them or is in a fluid: what
    /// they may jump or swim up from.
    fn supported(&self, feet: Vec3) -> bool {
        let blocks = &self.content.blocks;
        let ground = Aabb { min: feet - Vec3::new(dims::WIDTH / 2.0, 0.1, dims::WIDTH / 2.0), max: feet + Vec3::new(dims::WIDTH / 2.0, 0.0, dims::WIDTH / 2.0) };
        ground.intersects(|cell| self.is_solid(cell))
            || Aabb::standing(feet, dims::WIDTH, dims::HEIGHT).intersects(|cell| self.block(cell).is_some_and(|b| blocks.info(b).fluid.is_some()))
    }

    /// Whether a player moving straight from `from` to `to` would pass through a wall
    /// (with a slightly smaller box than theirs, for rounding). A player the world has
    /// closed in on (a block placed on them) is judged by where they end up only, so they
    /// can always step out.
    fn blocked_between(&self, from: Vec3, to: Vec3) -> bool {
        let inside = |feet: Vec3| Aabb::standing(feet, dims::WIDTH * 0.9, dims::HEIGHT * 0.95).intersects(|cell| self.is_solid(cell));
        if inside(from) {
            return inside(to);
        }
        let steps = ((to - from).length() / SWEEP_STEP).ceil().max(1.0) as u32;
        (1..=steps).any(|i| inside(from.lerp(to, i as f32 / steps as f32)))
    }

    fn settings(&mut self, id: PlayerId, view_distance: u8, fly_speed: f32) {
        let max_view_distance = self.config.view_distance;
        let player = self.players.get_mut(&id).unwrap();
        player.view_distance = (view_distance as i32).min(max_view_distance);
        player.fly_speed = if player.admin && fly_speed.is_finite() { fly_speed.clamp(1.0, MAX_FLY_SPEED) } else { 1.0 };
    }
}

/// A player's experience from a save, by skill name; skills that no longer exist are left
/// out.
fn restore_skills(saved: &[(String, u32)], registry: &SkillRegistry) -> Skills {
    let mut skills = Skills::default();
    for (name, xp) in saved {
        if let Some(id) = registry.id(name) {
            skills.grant(id, *xp);
        }
    }
    skills
}
