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
use voxmar_engine::movement::Checks;
use voxmar_engine::physics::{Aabb, player as dims};
use voxmar_engine::skill::{SkillRegistry, Skills};
use voxmar_engine::traits::{self, Health, TraitDecl};
use voxmar_engine::vitals::Vitals;
use voxmar_protocol::{
    ChatKind, ClientMessage, MAX_FLY_SPEED, PROTOCOL_VERSION, PlayerId, PlayerInput, PlayerView, ServerConnection, ServerMessage,
    TICKS_PER_SECOND,
};

use crate::{Limits, Server};
use crate::save::{GameMode, SavedPlayer};

pub struct Player {
    pub id: PlayerId,
    pub name: String,
    pub connection: ServerConnection,
    pub joined: bool,
    pub admin: bool,
    /// An admin by right (the integrated server's own player, a name on the admin list from
    /// this machine, or with the admin token), which `/deop` takes away only until they
    /// join again.
    pub admin_by_right: bool,
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
    /// Ground the player may still cover (or, flying, air), how far they may still rise
    /// and fall, in blocks, and when these were last topped up (`Server::input`): each
    /// refills at its top speed by the clock their messages arrive on, so a burst of inputs
    /// gets no more than the time they span.
    pub budgets: Budgets,
    pub budget_at: Option<Instant>,
    /// Where the player's own input last put them (the server moving them, by a teleport
    /// or a respawn, starts afresh); the height they last stood, swam or flew at, which on
    /// foot they may rise above by a jump only, whether that was in water, and when.
    pub accepted: Vec3,
    /// How many times the server has moved the player (`Player::teleport`); inputs that
    /// do not echo it are stale.
    pub teleports: u32,
    pub support: Support,
    /// The last place they stood on ground out of lava: where a death in the void or in
    /// lava leaves their things (`Server::grave_site`).
    pub last_ground: Option<Vec3>,
    /// The tick the player connected, for how long they may take to say hello, and when
    /// they last sent anything (`Player::idle`).
    pub connected_tick: u64,
    pub last_heard: Instant,
    /// Messages the player may still send this tick; lines of chat (and, for those not
    /// admins, commands) they may still send before having to slow down, as of when they
    /// last spoke, and whether they have been told to since.
    pub messages_left: u32,
    pub chat_allowance: f32,
    pub chat_at: Option<Instant>,
    pub told_to_slow: bool,
    /// The last tick they stood on something, on foot (a dodge needs it, or nearly).
    pub grounded: u64,
    /// Owner-replicated traits changed since they were last sent.
    pub dirty: Vec<TraitDecl>,
    pub sent: FxHashSet<ColumnPos>,
    /// Dropped items this player has been told about.
    pub items_seen: FxHashSet<u32>,
}

impl Player {
    pub fn new(id: PlayerId, connection: ServerConnection, spawn: Vec3, view_distance: i32, tick: u64, max_health: u8, limits: &Limits) -> Self {
        Self {
            id,
            name: String::new(),
            connection,
            joined: false,
            admin: false,
            admin_by_right: false,
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
            budgets: Budgets::default(),
            budget_at: None,
            accepted: spawn,
            teleports: 0,
            support: Support { y: spawn.y, fluid: false, at: None },
            last_ground: None,
            connected_tick: tick,
            last_heard: Instant::now(),
            messages_left: limits.messages_per_tick,
            chat_allowance: limits.chat_burst,
            chat_at: None,
            told_to_slow: false,
            grounded: 0,
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

    /// Moves the player to `position` and tells their client, numbering the teleport so
    /// inputs they sent before seeing it are dropped (`PlayerInput::teleport`).
    pub fn teleport(&mut self, position: Vec3) {
        self.position = position;
        self.teleports = self.teleports.wrapping_add(1);
        self.connection.send(ServerMessage::Teleport { position, seq: self.teleports });
    }

    /// Whether a player playing from elsewhere has sent nothing, not even the keepalive
    /// their client's network thread sends while the game itself is minimized or busy,
    /// for longer than `timeout` by `now`. The integrated server's own player never is.
    pub fn idle(&self, now: Instant, timeout: std::time::Duration) -> bool {
        let heard = self.connection.last_heard().map_or(self.last_heard, |at| at.max(self.last_heard));
        self.joined && !self.connection.is_local() && now.saturating_duration_since(heard) > timeout
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
        if let Some(player) = self.players.get_mut(&id) {
            player.last_heard = player.last_heard.max(at);
        }
        if self.players[&id].joined {
            self.complete_actions_of(id, at);
        }
        match message {
            ClientMessage::Hello { name, protocol, content, admin_token } => self.hello(id, name, protocol, content, admin_token),
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
            ClientMessage::Guard { up } => self.raise_guard(id, up),
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

    /// Whether a new player from `address` may have the starting kit: at most
    /// `kits_per_address` in any `kit_window`, by the server's ticks. Counts it if so.
    pub(crate) fn grant_kit(&mut self, address: std::net::IpAddr) -> bool {
        let address = self.config.limits.address_key(address);
        let window = (self.survival.kit_window * TICKS_PER_SECOND as f32) as u64;
        let tick = self.tick;
        let given = self.kits.entry(address).or_default();
        given.retain(|&at| tick.saturating_sub(at) < window);
        let granted = given.len() < self.survival.kits_per_address as usize;
        if granted {
            given.push(tick);
        }
        if given.is_empty() {
            self.kits.remove(&address);
        }
        granted
    }

    /// Forgets the addresses whose kits are all older than `kit_window`, so the list does
    /// not grow with every address that ever joined.
    pub(crate) fn forget_old_kits(&mut self) {
        let (window, tick) = ((self.survival.kit_window * TICKS_PER_SECOND as f32) as u64, self.tick);
        self.kits.retain(|_, given| {
            given.retain(|&at| tick.saturating_sub(at) < window);
            !given.is_empty()
        });
    }

    fn hello(&mut self, id: PlayerId, name: String, protocol: u32, content: u64, admin_token: Option<String>) {
        let fingerprint = self.content.fingerprint();
        let blocks = &self.content.blocks;
        let (seed, allow_flight) = (self.meta.seed, self.config.allow_flight);
        let name: String = name.chars().filter(|c| !c.is_control()).take(self.config.limits.name_length).collect();
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
        } else if self.players.values().any(|p| p.joined && same_name(&p.name, &name)) {
            Some(Words::key("net.rejected.name_taken").with("name", name.as_str()))
        } else if self.players.values().filter(|p| p.joined).count() >= self.config.max_players {
            Some(Words::key("net.rejected.full"))
        } else {
            None
        };
        if let Some(reason) = reason {
            self.players[&id].connection.send(ServerMessage::Rejected { reason });
            return;
        }
        // A save that is not a player file is moved aside rather than replaced by a new
        // player's at the next save. One that could not be read at all (no permission, a
        // failing disk), or cannot be moved, turns the player away.
        let (saved, set_aside) = match self.save.as_ref().map(|save| (save, save.read_player(&name, blocks))) {
            None => (None, None),
            Some((_, Ok(saved))) => (saved, None),
            Some((_, Err(e))) if e.kind() != std::io::ErrorKind::InvalidData => {
                tracing::error!("could not read player {name}: {e}");
                self.players[&id].connection.send(ServerMessage::Rejected { reason: Words::key("net.rejected.save") });
                return;
            }
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
        // A new player gets the starting kit, but an address gets only so many in a while
        // (`survival.ron`), so fresh names do not farm it.
        let kit = saved.is_none() && self.players[&id].connection.peer().is_none_or(|address| self.grant_kit(address));
        let new_inventory = self.survival.new_inventory(kit).expect("checked by load_content");
        let local_admin = self.config.local_admin;
        let token_admin = self.config.admin_token.as_deref().zip(admin_token.as_deref()).is_some_and(|(ours, theirs)| same_token(ours, theirs));
        let skill_ids = self.content.skills.clone();
        let effect_ids = self.content.effects.clone();
        let weather_seed = self.weather.seed();
        let player = self.players.get_mut(&id).expect("message from unknown player");

        player.name = name;
        player.joined = true;
        player.last_heard = Instant::now();
        // Only the integrated server's own player can connect in-process. Names are not
        // authenticated, so a name on the admin list counts only from this machine (a
        // tunnel that forwards from elsewhere makes everyone look local); from anywhere, the
        // admin token does. An admin makes others admins with `/op`, until they leave: a
        // name is not checked, so nothing lasts on it.
        player.admin_by_right = (player.connection.is_local() && local_admin)
            || (player.connection.is_loopback() && self.config.admins.iter().any(|admin| same_name(admin, &player.name)))
            || token_admin;
        player.admin = player.admin_by_right;
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
            // Players who left dead come back to life at home, or else the spawn point.
            Some(saved) => {
                player.home = saved.home.map(Vec3::from_array);
                player.bed = saved.bed.map(Vec3::from_array);
                if let Some(home) = player.home.or(player.bed) {
                    player.position = home;
                }
                player.inventory = saved.inventory;
                player.skills = restore_skills(&saved.skills, &skill_ids);
                player.journal = voxmar_engine::journal::Journal::from_entries(saved.journal);
                player.quests_done = saved.quests;
                player.tallies = saved.tallies.into_iter().collect();
            }
            // The coin purse in the pack's last slot, out of the hotbar's way, and the kit
            // unless this address has had its share.
            None => player.inventory = new_inventory,
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
        // Where they joined may be lava or the void; a grave waits only where they have
        // stood since (or at spawn).
        player.last_ground = None;
        tracing::info!("{} joined", player.name);
        let time = self.time_message();
        self.players[&id].connection.send(time);
        let entries = self.players[&id].journal.entries().cloned().collect();
        self.players[&id].connection.send(ServerMessage::Journal { entries });
        if let Some(file) = set_aside {
            let text = Words::key("save.player_set_aside").with("file", Words::text(file));
            self.players[&id].connection.send(ServerMessage::Chat { kind: ChatKind::Error, text });
        }
        if admin && self.generation_changed {
            self.players[&id].connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("world.generation_changed") });
        }
    }

    /// Gives every player their share of messages for the tick, and closes connections
    /// that have not said hello in time.
    pub(crate) fn limit_players(&mut self) {
        let (tick, limits) = (self.tick, &self.config.limits);
        let hello_ticks = (limits.hello_seconds * TICKS_PER_SECOND as f32) as u64;
        self.players.retain(|_, player| {
            player.messages_left = limits.messages_per_tick;
            player.joined || tick.saturating_sub(player.connected_tick) <= hello_ticks
        });
    }

    /// Whether the player may send a line of chat (or a command, unless an admin) now: their
    /// allowance refills by the clock their messages arrive on, so a server running behind
    /// does not cut it. Refused lines are dropped; the first of a run tells them to slow
    /// down.
    pub(crate) fn may_speak(&mut self, id: PlayerId, command: bool) -> bool {
        let at = self.arrival;
        let Some(player) = self.players.get_mut(&id) else { return false };
        if command && player.admin {
            return true;
        }
        let since = player.chat_at.map_or(0.0, |then| at.saturating_duration_since(then).as_secs_f32());
        let Limits { chat_burst, chat_per_second, .. } = self.config.limits;
        player.chat_allowance = (player.chat_allowance + since * chat_per_second).min(chat_burst);
        player.chat_at = Some(player.chat_at.map_or(at, |then| then.max(at)));
        if player.chat_allowance < 1.0 {
            if !player.told_to_slow {
                player.told_to_slow = true;
                player.connection.send(ServerMessage::Chat { kind: ChatKind::Error, text: Words::key("chat.too_fast") });
            }
            return false;
        }
        player.chat_allowance -= 1.0;
        player.told_to_slow = false;
        true
    }

    /// Accepts the client's predicted movement if it is physically plausible, otherwise
    /// snaps the client back. Plausible is within what their budgets allow (no faster
    /// across, up or down than their top speeds by the clock, however many inputs they
    /// send), on foot no higher above where they last stood or swam than a jump takes them
    /// (and coming down once they have been off the ground a while), and through no wall
    /// on the way.
    fn input(&mut self, id: PlayerId, input: PlayerInput) {
        let at = self.arrival;
        let player = &self.players[&id];
        if !player.health.alive() || !input.position.is_finite() || !input.yaw.is_finite() || !input.pitch.is_finite() {
            return;
        }
        // Sent before the client saw the server move them: judged from where they were
        // moved to, it could undo a short teleport, so it counts for nothing.
        if input.teleport != player.teleports {
            return;
        }
        // Quicker on foot as their effects say, as the client moves them.
        let tuning = &self.content.movement.on_foot(player.modifiers.speed.max(1.0));
        let checks = tuning.checks;
        let flying = input.flying && player.allow_flight;
        // Flying, across is as fast diagonally as the client flies (both keys held), and up
        // and down as fast as flight or a fall.
        let fly = tuning.fly_sprint_speed * player.fly_speed;
        let rates = if flying {
            Budgets {
                across: checks.speed_margin * fly * std::f32::consts::SQRT_2,
                rise: checks.speed_margin * fly.max(tuning.jump_velocity),
                fall: checks.speed_margin * fly.max(tuning.terminal_velocity),
            }
        } else {
            Budgets {
                across: checks.speed_margin * tuning.top_ground_speed(),
                rise: checks.speed_margin * tuning.jump_velocity,
                fall: checks.speed_margin * tuning.terminal_velocity,
            }
        };
        let gap = player.budget_at.map_or(1.0 / TICKS_PER_SECOND as f32, |then| at.saturating_duration_since(then).as_secs_f32());
        let since = gap.min(checks.burst);
        let from = player.position;
        // Moved by the server since their last input (a teleport, a respawn): afresh.
        let afresh = from != player.accepted || player.budget_at.is_none();
        let budgets = rates.refilled(&player.budgets, since, afresh, &checks);
        let mut support = if afresh || (player.flying && player.allow_flight) {
            Support { y: from.y, fluid: self.standing(from) == Some(true), at: Some(at) }
        } else {
            match self.standing(from) {
                Some(fluid) => Support { y: from.y, fluid, at: Some(at) },
                None => player.support,
            }
        };
        // The hover clock runs only while the player is seen off the ground: it restarts
        // when a hop has just touched down, and pauses across a gap in their inputs (a
        // stalled connection, a hitch), so silence neither sinks them nor buys more time.
        if self.near_ground(from) {
            support.at = Some(at);
        } else if gap > checks.hover_grace {
            let paused = std::time::Duration::from_secs_f32(gap);
            support.at = support.at.map(|then| (then + paused).min(at));
        }
        let delta = input.position - from;
        let cost = Budgets { across: delta.with_y(0.0).length(), rise: delta.y.max(0.0), fall: (-delta.y).max(0.0) };
        // Landing is always allowed: the height they may keep sinks only for a move that
        // ends in the air.
        let lands = self.standing(input.position).is_some();
        let ceiling = support.ceiling(tuning, at, !lands);
        let too_high = !flying && input.position.y > ceiling;
        let plausible = budgets.covers(&cost) && !too_high && self.clear_between(from, input.position);
        // Someone kept up past the height they may stay at is let down to it, a little
        // further each time, rather than left hanging where they were: onto the ground
        // below if the height is in it.
        let sunk = support.ceiling(tuning, at, true);
        let let_down = !plausible && !flying && from.y > sunk;
        let lowered = if !let_down {
            from
        } else if self.inside(Vec3::new(from.x, sunk, from.z)) {
            self.ground_below(from)
        } else {
            Vec3::new(from.x, sunk, from.z)
        };
        if let_down {
            // From there, the height they may keep pauses, then sinks on.
            support = support.let_down_to(lowered.y, tuning, at);
        }

        let player = self.players.get_mut(&id).unwrap();
        player.yaw = input.yaw;
        player.pitch = input.pitch.clamp(-1.6, 1.6);
        player.flying = flying;
        player.selected = (input.selected_slot as usize).min(HOTBAR_SLOTS - 1);
        player.budget_at = Some(at);
        player.support = support;
        if plausible {
            player.budgets = budgets.less(&cost);
            player.position = input.position;
            player.accepted = input.position;
        } else {
            player.budgets = budgets;
            if let_down && lowered != from {
                player.accepted = lowered;
            }
            let back = if let_down { lowered } else { player.position };
            player.teleport(back);
        }
    }

    /// Where a player who died leaves their things: where they fell, unless that was in
    /// the void or in lava, where nobody could come back for them; then the last place
    /// they stood (`Player::last_ground`), or the world's spawn.
    pub(crate) fn grave_site(&self, id: PlayerId) -> Vec3 {
        let player = &self.players[&id];
        let blocks = &self.content.blocks;
        let in_lava = player.aabb().intersects(|cell| self.block(cell).and_then(|b| blocks.fluid(b)).is_some_and(|f| crate::process::is_lava(blocks, f.fluid)));
        if player.position.y < self.survival.void_below || in_lava { player.last_ground.unwrap_or(self.spawn) } else { player.position }
    }

    /// What a player standing at `feet` has under them: `Some(true)` a fluid they are in,
    /// `Some(false)` ground, `None` nothing. What they may jump or swim up from. Where the
    /// server has not loaded the land yet, it takes the player's word.
    fn standing(&self, feet: Vec3) -> Option<bool> {
        let blocks = &self.content.blocks;
        if !self.world.contains(ColumnPos::of_block(feet.floor().as_ivec3())) {
            return Some(false);
        }
        let ground = Aabb { min: feet - Vec3::new(dims::WIDTH / 2.0, 0.1, dims::WIDTH / 2.0), max: feet + Vec3::new(dims::WIDTH / 2.0, 0.0, dims::WIDTH / 2.0) };
        // In water, even on its bottom, a swimmer may be lifted out onto a bank: water as the
        // client swims in it (`movement::water_around`), so not lava.
        let (depth, _) = voxmar_engine::movement::water_around(blocks, |cell| self.block(cell), &Aabb::standing(feet, dims::WIDTH, dims::HEIGHT));
        if depth > 0.0 {
            Some(true)
        } else if ground.intersects(|cell| self.is_solid(cell)) {
            Some(false)
        } else {
            None
        }
    }

    /// Whether there is ground within `Checks::near_ground` under a player at `feet`.
    fn near_ground(&self, feet: Vec3) -> bool {
        let half = dims::WIDTH / 2.0;
        let below = Aabb { min: feet - Vec3::new(half, self.content.movement.checks.near_ground, half), max: feet + Vec3::new(half, 0.0, half) };
        below.intersects(|cell| self.is_solid(cell))
    }

    /// Where a player above `from` would stand on the first solid block under them (within
    /// 64 blocks; otherwise where they are).
    fn ground_below(&self, from: Vec3) -> Vec3 {
        let cell = from.floor().as_ivec3();
        (0..64)
            .map(|down| cell - voxmar_engine::glam::IVec3::Y * down)
            .find(|&below| self.is_solid(below) && !self.inside(below.as_vec3().with_x(from.x).with_z(from.z) + Vec3::Y))
            .map_or(from, |below| Vec3::new(from.x, below.y as f32 + 1.0, from.z))
    }

    /// Whether a player at `feet` would be in a wall (with a slightly smaller box than
    /// theirs, for rounding: `Checks::wall_box`).
    fn inside(&self, feet: Vec3) -> bool {
        let (width, height) = self.content.movement.checks.wall_box;
        Aabb::standing(feet, dims::WIDTH * width, dims::HEIGHT * height).intersects(|cell| self.is_solid(cell))
    }

    /// Whether a player could have got from `from` to `to` without passing through a wall.
    /// Between two inputs the client moved them along some curve: off a ledge they stay
    /// high and then drop, onto a step they rise and then go on, so the straight line can
    /// cut a corner they went round. A way is clear if the straight line is, or the path
    /// across and then up or down, or up or down and then across (the client moves along
    /// one axis at a time). A player the world has closed in on (a block placed on them)
    /// is judged by where they end up only, so they can always step out.
    fn clear_between(&self, from: Vec3, to: Vec3) -> bool {
        if self.inside(from) {
            return !self.inside(to);
        }
        let sweep_step = self.content.movement.checks.sweep_step;
        let clear = |a: Vec3, b: Vec3| {
            let steps = ((b - a).length() / sweep_step).ceil().max(1.0) as u32;
            (1..=steps).all(|i| !self.inside(a.lerp(b, i as f32 / steps as f32)))
        };
        let across_first = Vec3::new(to.x, from.y, to.z);
        let up_first = Vec3::new(from.x, to.y, from.z);
        clear(from, to) || (clear(from, across_first) && clear(across_first, to)) || (clear(from, up_first) && clear(up_first, to))
    }

    fn settings(&mut self, id: PlayerId, view_distance: u8, fly_speed: f32) {
        let max_view_distance = self.config.view_distance;
        let player = self.players.get_mut(&id).unwrap();
        player.view_distance = (view_distance as i32).min(max_view_distance);
        player.fly_speed = if player.admin && fly_speed.is_finite() { fly_speed.clamp(1.0, MAX_FLY_SPEED) } else { 1.0 };
    }
}

/// Blocks a player may still move (`Player::budgets`), or move a second, or did move:
/// across (or, flying, through the air), up and down.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Budgets {
    pub across: f32,
    pub rise: f32,
    pub fall: f32,
}

impl Budgets {
    /// These rates' budgets after `since` seconds more of `left`, each up to
    /// `Checks::burst` seconds' worth and a little slack; full when starting afresh.
    fn refilled(&self, left: &Budgets, since: f32, afresh: bool, checks: &Checks) -> Budgets {
        let fill = |left: f32, rate: f32| {
            let cap = rate * checks.burst + checks.slack;
            if afresh { cap } else { (left + since * rate).min(cap) }
        };
        Budgets { across: fill(left.across, self.across), rise: fill(left.rise, self.rise), fall: fill(left.fall, self.fall) }
    }

    fn covers(&self, cost: &Budgets) -> bool {
        cost.across <= self.across && cost.rise <= self.rise && cost.fall <= self.fall
    }

    fn less(&self, cost: &Budgets) -> Budgets {
        Budgets { across: self.across - cost.across, rise: self.rise - cost.rise, fall: self.fall - cost.fall }
    }
}

/// Where a player last had something under them (`Player::support`): the height, whether
/// it was water rather than ground, and when.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Support {
    pub y: f32,
    pub fluid: bool,
    pub at: Option<Instant>,
}

impl Support {
    /// The highest a player on foot may be `now`: a jump above where they last stood (and
    /// out of water, the step onto a bank besides), sinking once they have been off the
    /// ground longer than a jump or a fall from it takes.
    fn ceiling(&self, tuning: &voxmar_engine::movement::Tuning, now: Instant, sinking: bool) -> f32 {
        let aloft = self.at.map_or(0.0, |at| now.saturating_duration_since(at).as_secs_f32());
        let checks = &tuning.checks;
        let sink = if sinking { (aloft - checks.hover_grace).max(0.0) * checks.hover_sink } else { 0.0 };
        self.y + Self::rise(tuning, self.fluid) - sink
    }

    /// How far above where they stood a player may rise: a jump and slack, and out of
    /// water the step onto a bank.
    fn rise(tuning: &voxmar_engine::movement::Tuning, fluid: bool) -> f32 {
        let jump = tuning.jump_velocity * tuning.jump_velocity / (2.0 * tuning.gravity);
        jump.max(tuning.step_up) + tuning.checks.rise_slack + if fluid { tuning.step_out } else { 0.0 }
    }

    /// The support of a player let down to `y` at `now`: the height they may keep is `y`,
    /// for `Checks::let_down_pause`, and sinks on from there.
    fn let_down_to(&self, y: f32, tuning: &voxmar_engine::movement::Tuning, now: Instant) -> Support {
        let pause = std::time::Duration::from_secs_f32(tuning.checks.hover_grace - tuning.checks.let_down_pause);
        Support { y: y - Self::rise(tuning, false), fluid: false, at: Some(now.checked_sub(pause).unwrap_or(now)) }
    }
}

/// Whether a token given is the server's, compared in time that does not depend on where
/// they first differ.
fn same_token(ours: &str, theirs: &str) -> bool {
    ours.len() == theirs.len() && ours.bytes().zip(theirs.bytes()).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

/// Whether two names are one player's: names differ in case only for show.
pub(crate) fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
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
