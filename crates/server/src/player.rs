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
    ClientMessage, MAX_FLY_SPEED, PROTOCOL_VERSION, PlayerId, PlayerInput, PlayerView, ServerConnection, ServerMessage,
    TICKS_PER_SECOND,
};

use crate::Server;
use crate::save::{GameMode, SavedPlayer};

pub const MAX_HEALTH: u8 = 20;

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
    /// The most health the Hitpoints level gives, before effects (0 until first worked out).
    pub health_base: u8,
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
    pub last_input_tick: u64,
    /// Owner-replicated traits changed since they were last sent.
    pub dirty: Vec<TraitDecl>,
    pub sent: FxHashSet<ColumnPos>,
    /// Dropped items this player has been told about.
    pub items_seen: FxHashSet<u32>,
}

impl Player {
    pub fn new(id: PlayerId, connection: ServerConnection, spawn: Vec3, view_distance: i32) -> Self {
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
            health: Health::full(MAX_HEALTH),
            inventory: Inventory::default(),
            skills: Skills::default(),
            vitals: Vitals::default(),
            timed_effects: Vec::new(),
            modifiers: Modifiers::default(),
            recent_food: Default::default(),
            health_base: 0,
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
            last_input_tick: 0,
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
            ClientMessage::Command { line } => self.command(id, &line),
            ClientMessage::Chat { text } => self.chat(id, &text),
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
        let start_kit: Vec<(BlockId, u8)> =
            [("base:oak_planks", 64), ("base:cobblestone", 64), ("base:glass", 32), ("base:glowstone", 16)]
                .into_iter()
                .filter_map(|(name, count)| blocks.id(name).map(|id| (id, count)))
                .collect();
        let purse = blocks.id("base:coin_purse");
        let (seed, allow_flight, tick) = (self.meta.seed, self.config.allow_flight, self.tick);
        let name: String = name.chars().filter(|c| !c.is_control()).take(32).collect();
        let name = if name.trim().is_empty() { "player".to_owned() } else { name };
        let saved = self.save.as_ref().and_then(|save| {
            save.read_player(&name, blocks).unwrap_or_else(|e| {
                tracing::error!("could not load player {name}: {e}");
                None
            })
        });
        let local_admin = self.config.local_admin;
        let skill_ids = self.content.skills.clone();
        let effect_ids = self.content.effects.clone();
        let weather_seed = self.weather.seed();
        let player = self.players.get_mut(&id).expect("message from unknown player");
        if player.joined {
            return;
        }
        let reason = if protocol != PROTOCOL_VERSION {
            Some(Words::key("net.rejected.protocol").with("theirs", Words::text(protocol)).with("ours", Words::text(PROTOCOL_VERSION)))
        } else if content != fingerprint {
            Some(Words::key("net.rejected.content"))
        } else {
            None
        };
        if let Some(reason) = reason {
            player.connection.send(ServerMessage::Rejected { reason });
            return;
        }

        player.name = name;
        player.joined = true;
        // Only the integrated server's own player can connect in-process.
        player.admin = (player.connection.is_local() && local_admin) || self.config.admins.contains(&player.name);
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
                // A coin purse, in the pack's last slot, out of the hotbar's way.
                if let Some(purse) = purse {
                    player.inventory.slots[voxmar_engine::inventory::SLOTS - 1] = Some(ItemStack { item: purse, count: 1 });
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
        player.last_input_tick = tick;
        tracing::info!("{} joined", player.name);
        let time = self.time_message();
        self.players[&id].connection.send(time);
        let entries = self.players[&id].journal.entries().cloned().collect();
        self.players[&id].connection.send(ServerMessage::Journal { entries });
    }

    /// Accepts the client's predicted movement if it is physically plausible, otherwise
    /// snaps the client back.
    fn input(&mut self, id: PlayerId, input: PlayerInput) {
        let tick = self.tick;
        let player = &self.players[&id];
        if !player.health.alive() || !input.position.is_finite() {
            return;
        }
        let allow_flight = player.allow_flight;
        let elapsed = ((tick - player.last_input_tick) as f32 / TICKS_PER_SECOND as f32).clamp(0.05, 1.0);
        let delta = input.position - player.position;
        let flying = input.flying && allow_flight;
        let tuning = &self.content.movement;
        let fly_speed = tuning.fly_sprint_speed * player.fly_speed;
        let max_horizontal = if flying { fly_speed } else { tuning.top_ground_speed() } * 1.5 * elapsed + 0.5;
        let max_vertical = if flying { fly_speed } else { tuning.terminal_velocity } * 1.5 * elapsed + 1.5;
        let aabb = Aabb::standing(input.position, dims::WIDTH * 0.9, dims::HEIGHT * 0.95);
        let inside_wall = aabb.intersects(|p| self.is_solid(p));
        let plausible = delta.with_y(0.0).length() <= max_horizontal && delta.y.abs() <= max_vertical && !inside_wall;

        let player = self.players.get_mut(&id).unwrap();
        player.yaw = input.yaw;
        player.pitch = if input.pitch.is_finite() { input.pitch.clamp(-1.6, 1.6) } else { 0.0 };
        player.flying = flying;
        player.selected = (input.selected_slot as usize).min(HOTBAR_SLOTS - 1);
        if plausible {
            player.position = input.position;
            player.last_input_tick = tick;
        } else {
            player.connection.send(ServerMessage::Teleport { position: player.position });
        }
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
