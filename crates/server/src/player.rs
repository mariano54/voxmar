//! Players: agents whose controller is a remote input device. A player's session handles
//! the handshake and validates movement; everything else they do goes through actions.

use std::time::Instant;

use rustc_hash::FxHashSet;
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::effect::{EffectId, EffectRegistry, Modifiers};
use voxmar_engine::glam::Vec3;
use voxmar_engine::inventory::{HOTBAR_SLOTS, Inventory, ItemStack, max_stack};
use voxmar_engine::lang::Words;
use voxmar_engine::movement::Checks;
use voxmar_engine::physics::{Aabb, CUBE, player as dims};
use voxmar_engine::skill::{SkillRegistry, Skills};
use voxmar_engine::traits::{self, Health, TraitDecl};
use voxmar_engine::vitals::Vitals;
use voxmar_protocol::{
    ChatKind, ClientMessage, MAX_FLY_SPEED, PROTOCOL_VERSION, PlayerId, PlayerInput, PlayerView, ServerConnection, ServerMessage,
    TICKS_PER_SECOND,
};

use crate::{Limits, Server};
use crate::save::{Draw, Draws, GameMode, SavedPlayer};

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
    /// The store the player has open (`stores`), whose slots follow theirs.
    pub store: Option<voxmar_engine::glam::IVec3>,
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
    /// The announced effects (`EffectDef::announce`) the player has been told of this
    /// session, so stepping out of a room and back in does not say it twice.
    pub announced: Vec<EffectId>,
    /// The home steps their home met when last surveyed (`EffectRegistry::home_steps`),
    /// where that home stood (its `home_spot`'s cell: a home moved since says nothing of
    /// what was lost), and whether it is yet to be surveyed since they joined, to tell them
    /// what lights burnt out while they were away cost.
    pub home_steps: Vec<EffectId>,
    pub home_anchor: Option<voxmar_engine::glam::IVec3>,
    pub home_unchecked: bool,
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
    /// Whether their home hearth needs no lighting again from an older save
    /// (`hearth::kindle_home`): new players' and those already seen to.
    pub kindled: bool,
    /// Whether they have been told this session why something of theirs smoulders
    /// (`blaze_hint`).
    pub told_fire: bool,
    pub buried: bool,
    /// Everything the player has discovered (`journal`).
    pub journal: voxmar_engine::journal::Journal,
    /// Quests finished, those of them whose rewards were claimed (`quests::claim_quest`),
    /// tallies of what quests count (`crafted:base:torch`), and the quest states last sent
    /// (`quests`).
    pub quests_done: Vec<String>,
    pub quests_claimed: Vec<String>,
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
    /// The others as last told to this player (`Server::broadcast_players`), names and all.
    pub told: rustc_hash::FxHashMap<PlayerId, PlayerView>,
    /// The others' names as told to this player, near or far (`Server::broadcast_players`).
    pub named: rustc_hash::FxHashMap<PlayerId, String>,
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
    /// The ping awaiting its answer (its number, and when it went), and the round trip to
    /// the player's client as measured, smoothed (`fight`'s lag allowance).
    pub ping: Option<(u32, std::time::Instant)>,
    pub round_trip: Option<f32>,
    /// The last tick a blow fell between them and a creature, either way (prayer points
    /// come back only after a calm, `prayers`).
    pub last_blow: u64,
    /// The last tick a wound fell on them (`Player::wound`: a blow, a fall, a sting; not the
    /// cold or heat wearing at them): no sleeping off a fight just had (`survival.ron`'s
    /// `sleep_after_hurt`).
    pub hurt_at: Option<u64>,
    /// Prayer points and the prayers on, and what the player was last told of them.
    pub praying: voxmar_engine::prayer::Praying,
    /// Lines the player has cast, which numbers each cast's draw (`Server::fish`).
    pub casts: u64,
    /// What the player's other rolls are numbered by (`Draws::next`): strikes, blows taken,
    /// feeds and cooks.
    pub draws: Draws,
    /// The player's own share of the world's draw key (`player_draw_key`), so two players
    /// casting the same count into the same water draw apart, whatever connection each has.
    pub draw_key: u64,
    pub prayers_sent: Option<(u16, u16, Vec<voxmar_engine::prayer::PrayerId>)>,
    /// Owner-replicated traits changed since they were last sent.
    pub dirty: Vec<TraitDecl>,
    pub sent: FxHashSet<ColumnPos>,
    /// Dropped items this player has been told about.
    pub items_seen: FxHashSet<u32>,
    /// What their kept animals laid into the home's stores (`Product::nests`), by item
    /// name, not yet told: they are told once home.
    pub nested: Vec<(String, u32)>,
}

impl Player {
    /// The player's next two draws of `kind`, by their draw key and their count of that
    /// kind (`Draws`), counted.
    pub(crate) fn draw(&mut self, kind: Draw) -> [f32; 2] {
        let key = self.draw_key;
        self.draws.next(key, kind)
    }

    /// Where home is, the point a block above where the player stands on waking: the
    /// hearth they made home while it burns (in `lit`, by its core), else the bed they last
    /// slept in. A hearth broken or gone cold is no home; one from a save made before
    /// hearths kept their fire is home until it is lit again for them (`kindle_home`).
    /// Respawn, `/home`, the HUD, home effects and stores and wanderers all ask here.
    pub fn home_spot(&self, lit: &rustc_hash::FxHashMap<voxmar_engine::glam::IVec3, crate::hearth::Lit>) -> Option<Vec3> {
        self.home.filter(|&at| !self.kindled || lit.contains_key(&(at - Vec3::new(0.5, 1.0, 0.5)).round().as_ivec3())).or(self.bed)
    }

    /// The block home stands on (`home_spot`), as `Vitals::home` tells it.
    pub fn home_block(&self, lit: &rustc_hash::FxHashMap<voxmar_engine::glam::IVec3, crate::hearth::Lit>) -> Option<voxmar_engine::glam::IVec3> {
        self.home_spot(lit).map(|at| (at - Vec3::Y).floor().as_ivec3())
    }

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
            store: None,
            flying: false,
            health: Health::full(max_health),
            inventory: Inventory::default(),
            skills: Skills::default(),
            vitals: Vitals::default(),
            timed_effects: Vec::new(),
            announced: Vec::new(),
            home_steps: Vec::new(),
            home_anchor: None,
            home_unchecked: false,
            modifiers: Modifiers::default(),
            recent_food: Default::default(),
            health_base: 0,
            home: None,
            bed: None,
            kindled: true,
            told_fire: false,
            buried: false,
            journal: Default::default(),
            quests_done: Vec::new(),
            quests_claimed: Vec::new(),
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
            told: rustc_hash::FxHashMap::default(),
            named: rustc_hash::FxHashMap::default(),
            support: Support { y: spawn.y, fluid: false, at: None },
            last_ground: None,
            connected_tick: tick,
            last_heard: Instant::now(),
            messages_left: limits.messages_per_tick,
            chat_allowance: limits.chat_burst,
            chat_at: None,
            told_to_slow: false,
            grounded: 0,
            ping: None,
            round_trip: None,
            last_blow: 0,
            hurt_at: None,
            praying: Default::default(),
            casts: 0,
            draws: Draws::default(),
            draw_key: 0,
            prayers_sent: None,
            dirty: vec![traits::HEALTH, traits::INVENTORY, traits::SKILLS, traits::VITALS],
            sent: FxHashSet::default(),
            items_seen: FxHashSet::default(),
            nested: Vec::new(),
        }
    }

    pub fn view(&self) -> PlayerView {
        PlayerView { id: self.id, name: Some(self.name.clone()), position: self.position, yaw: self.yaw, pitch: self.pitch, held: self.held(), fight: Default::default() }
    }

    pub fn aabb(&self) -> Aabb {
        Aabb::standing(self.position, dims::WIDTH, dims::HEIGHT)
    }

    pub fn eye(&self) -> Vec3 {
        voxmar_engine::vitals::eye(self.position, dims::EYE_HEIGHT, self.vitals.seated)
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
    /// `amount` health taken by a wound at `tick` (a blow, a fall, a sting), which keeps them
    /// from sleeping a while even if it heals back at once; true if it killed them.
    pub fn wound(&mut self, amount: u8, tick: u64) -> bool {
        if amount > 0 {
            self.hurt_at = Some(tick);
        }
        self.health.damage(amount)
    }

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
            kindled: self.kindled,
            journal: self.journal.entries().cloned().collect(),
            quests: self.quests_done.clone(),
            quests_claimed: self.quests_claimed.clone(),
            tallies: { let mut t: Vec<(String, u32)> = self.tallies.iter().map(|(k, &v)| (k.clone(), v)).collect(); t.sort(); t },
            prayer_points: Some(self.praying.points),
            casts: self.casts,
            draws: self.draws,
            home_steps: self.home_steps.iter().map(|&id| effects.get(id).name.clone()).collect(),
            home_anchor: self.home_anchor.map(|at| at.to_array()),
            spilled: Vec::new(),
        }
    }
}

impl Server {
    /// Handles a message that arrived `at`. The player's timed action due by then
    /// completes first: the player saw it done before sending this.
    pub(crate) fn handle_message(&mut self, id: PlayerId, message: ClientMessage, at: Instant) {
        self.arrival = at;
        // A fire this sets off is theirs.
        self.fire_origin = self.players.get(&id).map(|p| crate::player_key(&p.name));
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
                    // What a command changes is no one's doing: its fires are no one's, and it
                    // forgets no one's home hearth.
                    self.fire_origin = None;
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
            ClientMessage::Pong { id: number } => self.pong(id, number, at),
            ClientMessage::Pray { prayer, on } => self.pray(id, prayer, on),
            ClientMessage::LookedUp { item } => self.looked_up(id, item),
            ClientMessage::ClaimQuest { name } => self.claim_quest(id, &name),
            ClientMessage::LookAtStation { block } => self.look_at_station(id, block),
        }
        // What they did that set something smouldering, they are told why.
        self.blaze_hint(id);
        self.fire_origin = None;
    }

    /// A player looked an item up in the recipe viewer: counted once for quests, if the item
    /// exists.
    fn looked_up(&mut self, id: PlayerId, item: BlockId) {
        if (item.0 as usize) < self.content.blocks.len() && !item.is_air() {
            let name = self.content.blocks.def(item).name.clone();
            let player = self.players.get_mut(&id).expect("a joined player");
            player.tallies.entry(format!("lookedup:{name}")).or_insert(1);
        }
    }

    /// Creative mode's catalogue: `count` of any item onto the pointer (topping up a stack
    /// of it there) or into the inventory, where what does not fit is not given. A survival
    /// player gets nothing.
    fn take_from_catalogue(&mut self, id: PlayerId, item: BlockId, count: u8, held: bool) {
        // Not trusting the client's list: only what a player could hold, a merged-away plant
        // as the item it merged into.
        let item = if (item.0 as usize) < self.content.blocks.len() { self.content.blocks.item_of(item) } else { BlockId::AIR };
        let known = self.content.blocks.holdable(item);
        let Some(player) = self.players.get_mut(&id) else { return };
        if !known || count == 0 || player.mode() != GameMode::Creative {
            return;
        }
        // No more than a stack of it holds (a tool comes one at a time), and never onto a
        // worn one: a new one is not a free mend.
        let most = max_stack(item);
        let count = count.min(most);
        if held {
            match &mut player.inventory.cursor {
                None => player.inventory.cursor = Some(ItemStack::new(item, count)),
                Some(stack) if stack.item == item && stack.wear == 0 => stack.count = stack.count.saturating_add(count).min(most),
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

    /// Whether two connections come from one place: both in process, or both from one
    /// address as the limits count them (`Limits::address_key`).
    fn same_place(&self, a: PlayerId, b: PlayerId) -> bool {
        let (a, b) = (&self.players[&a].connection, &self.players[&b].connection);
        let key = |ip| self.config.limits.address_key(ip);
        match (a.peer(), b.peer()) {
            (Some(x), Some(y)) => key(x) == key(y),
            (None, None) => a.is_local() && b.is_local(),
            _ => false,
        }
    }

    pub(crate) fn hello(&mut self, id: PlayerId, name: String, protocol: u32, content: u64, admin_token: Option<String>) {
        let fingerprint = self.content.fingerprint();
        let blocks = self.content.blocks.clone();
        let (seed, allow_flight) = (self.meta.seed, self.config.allow_flight);
        let name: String = name.chars().filter(|c| !c.is_control()).take(self.config.limits.name_length).collect();
        let name = if name.trim().is_empty() { "player".to_owned() } else { name };
        if self.players[&id].joined {
            return;
        }
        // One player per name: a second login would load the same save twice, so whatever
        // either carried would be doubled and their saves would overwrite each other. Unless
        // it comes from where the first did, and the first has gone silent: a client that
        // froze or lost its network without closing its socket still looks joined until it
        // is idle too long, and its player coming back is them, so the old connection leaves
        // (saved) and this one joins. A session still heard is never ended so (an address
        // can be a whole school's); the integrated server's own player always comes back.
        let earlier = self.players.iter().find(|(_, p)| p.joined && same_name(&p.name, &name)).map(|(&earlier, _)| earlier);
        let silence = std::time::Duration::from_secs_f32(self.config.limits.rejoin_silence);
        let again = earlier.filter(|&earlier| {
            let local = self.players[&earlier].connection.is_local();
            self.same_place(earlier, id) && (local || self.players[&earlier].idle(Instant::now(), silence))
        });
        let reason = if protocol != PROTOCOL_VERSION {
            Some(Words::key("net.rejected.protocol").with("theirs", Words::text(protocol)).with("ours", Words::text(PROTOCOL_VERSION)))
        } else if content != fingerprint {
            Some(Words::key("net.rejected.content"))
        } else if earlier.is_some() && again.is_none() {
            Some(Words::key("net.rejected.name_taken").with("name", name.as_str()))
        } else if again.is_none() && self.players.values().filter(|p| p.joined).count() >= self.config.max_players {
            Some(Words::key("net.rejected.full"))
        } else {
            None
        };
        if let Some(reason) = reason {
            self.players[&id].connection.send(ServerMessage::Rejected { reason });
            return;
        }
        // The new connection takes what the earlier one held in memory, not the player's
        // file: leaving saves the whole world, and a save that fails (a disk that will not
        // take the file) leaves the old file behind, which would hand back what they have
        // dropped or stored since. The earlier spelling of the name stays theirs.
        let carried = again.map(|earlier| {
            tracing::info!("{name} joined again from the same address; the earlier, silent connection leaves");
            self.players[&earlier].connection.send(ServerMessage::Rejected { reason: Words::key("net.rejected.joined_again") });
            self.bury_if_dead(earlier);
            let (skills, effects) = (self.content.skills.clone(), self.content.effects.clone());
            let held = self.players[&earlier].saved(&skills, &effects);
            let spelling = self.players[&earlier].name.clone();
            self.leave(earlier);
            (held, spelling)
        });
        let name = carried.as_ref().map_or(name, |(_, spelling)| spelling.clone());
        // A save that is not a player file is moved aside rather than replaced by a new
        // player's at the next save. One that could not be read at all (no permission, a
        // failing disk), or cannot be moved, turns the player away.
        let (mut saved, set_aside) = if let Some((held, _)) = carried {
            (Some(held), None)
        } else {
            match self.save.as_ref().map(|save| (save, save.read_player(&name, &blocks))) {
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
            }
        };
        // A player plays under the name as first saved, whatever case they type now, so
        // everything that compares names (animals kept, drops left, machines owned) agrees.
        // Read it only after their full save has passed the checks above.
        let name = self.canonical_name(name, saved.is_some());
        // A new player gets the starting kit, but an address gets only so many in a while
        // (`survival.ron`), so fresh names do not farm it.
        let kit = saved.is_none() && self.players[&id].connection.peer().is_none_or(|address| self.grant_kit(address));
        let new_inventory = self.survival.new_inventory(kit).expect("checked by load_content");
        let local_admin = self.config.local_admin;
        let token_admin = self.config.admin_token.as_deref().zip(admin_token.as_deref()).is_some_and(|(ours, theirs)| same_token(ours, theirs));
        let skill_ids = self.content.skills.clone();
        let effect_ids = self.content.effects.clone();
        let prayers = self.content.prayers.clone();
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
        // Returning players carry on where they left, prayer points and all (the dead with
        // them full); new ones start at spawn with a kit.
        let points = saved.as_ref().filter(|saved| saved.health.alive()).and_then(|saved| saved.prayer_points);
        // Casts and the other rolls go on being numbered from where they were, alive or dead,
        // so none is drawn twice.
        player.casts = saved.as_ref().map_or(0, |saved| saved.casts);
        player.draws = saved.as_ref().map_or_else(Draws::default, |saved| saved.draws);
        player.draw_key = player_draw_key(self.meta.draw_key, &player.name);
        let spilled = saved.as_mut().map(|saved| std::mem::take(&mut saved.spilled)).unwrap_or_default();
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
                player.kindled = saved.kindled;
                player.journal = voxmar_engine::journal::Journal::from_entries(saved.journal);
                player.quests_done = saved.quests;
                player.quests_claimed = saved.quests_claimed;
                player.tallies = saved.tallies.into_iter().collect();
                player.timed_effects = saved.effects.iter().filter_map(|(name, left)| Some((effect_ids.id(name)?, *left))).collect();
                player.home_steps = saved.home_steps.iter().filter_map(|name| effect_ids.id(name)).collect();
                player.home_anchor = saved.home_anchor.map(voxmar_engine::glam::IVec3::from_array);
                player.home_unchecked = !player.home_steps.is_empty();
            }
            // Players who left dead come back to life at home, or else the spawn point.
            Some(saved) => {
                player.home = saved.home.map(Vec3::from_array);
                player.bed = saved.bed.map(Vec3::from_array);
                player.kindled = saved.kindled;
                if let Some(home) = player.home_spot(&self.lit) {
                    player.position = home;
                }
                player.inventory = saved.inventory;
                player.skills = restore_skills(&saved.skills, &skill_ids);
                player.journal = voxmar_engine::journal::Journal::from_entries(saved.journal);
                player.quests_done = saved.quests;
                player.quests_claimed = saved.quests_claimed;
                player.tallies = saved.tallies.into_iter().collect();
                player.home_steps = saved.home_steps.iter().filter_map(|name| effect_ids.id(name)).collect();
                player.home_anchor = saved.home_anchor.map(voxmar_engine::glam::IVec3::from_array);
                player.home_unchecked = !player.home_steps.is_empty();
            }
            // The coin purse in the pack's last slot, out of the hotbar's way, and the kit
            // unless this address has had its share.
            None => player.inventory = new_inventory,
        }
        skill_ids.start(&mut player.skills);
        let most = prayers.most(prayers.skill.map_or(1, |skill| skill_ids.level(player.skills.xp(skill))));
        player.praying.points = points.unwrap_or(most).clamp(0.0, most);
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
        self.kindle_home(id);
        self.recount_placed(id);
        // An older save's stacked tools with no slot left fall at their feet, theirs and
        // lying as long as a grave does: the pack was full, so they wait for room.
        let feet = self.players[&id].position.floor().as_ivec3();
        let owner = crate::items::owner_of(&self.players[&id].name);
        let lifetime = (self.content.value.death.grave * voxmar_protocol::TICKS_PER_SECOND as f32) as u32;
        for (i, stack) in spilled.into_iter().enumerate() {
            let random = self.meta.next_pop(feet, 0x5b1d ^ i as u32);
            self.items.spawn_for(stack, voxmar_engine::dropped::popped(feet, random), voxmar_engine::dropped::PICKUP_DELAY, Some(owner), lifetime);
        }
        self.players[&id].connection.send(ServerMessage::Fires { lit: self.fires() });
        // What smoulders now: the others were told as it began, and are told only of changes.
        self.players[&id].connection.send(ServerMessage::Smoulders { at: self.smouldering() });
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
        // Counted in ticks, so a server running behind does not cut off a slow hello; a server
        // whose ticks run quicker than the clock (`tick_speedup`) counts as many more.
        let hello_ticks = (limits.hello_seconds * TICKS_PER_SECOND as f32) as u64 * self.config.tick_speedup.max(1) as u64;
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

    /// Recover a legacy player's spelling from the ownership that still knows it, and
    /// fold aliases left by older case-changing logins. New saves' explicit name wins.
    fn canonical_name(&mut self, requested: String, saved: bool) -> String {
        let mut owners: Vec<String> = self.creatures.live.values()
            .flat_map(|c| c.owner.iter().chain(&c.trusting))
            .chain(self.creatures.unplaced.iter().map(|c| &c.owner))
            .chain(self.factory.iter().filter_map(|(_, _, s)| s.owner.as_ref()))
            .filter(|name| same_name(name, &requested)).cloned().collect();
        owners.sort();
        owners.dedup();
        let files = if saved { self.save.as_ref().map(|save| save.older_player_names(&requested)).unwrap_or_default() } else { Vec::new() };
        let canonical = saved.then(|| self.save.as_ref().and_then(|save| save.saved_name(&requested))).flatten()
            .or_else(|| owners.first().cloned())
            .or_else(|| files.first().cloned())
            .unwrap_or_else(|| requested.clone());
        let mut hashes: Vec<_> = owners.iter().chain(&files).chain(std::iter::once(&requested)).map(|name| crate::items::owner_of(name)).collect();
        hashes.sort_unstable();
        hashes.dedup();
        self.items.canonical_owner(&hashes, crate::items::owner_of(&canonical));
        for creature in self.creatures.live.values_mut() {
            for name in [&mut creature.owner, &mut creature.trusting].into_iter().flatten() {
                if name != &canonical && same_name(name, &canonical) {
                    name.clone_from(&canonical);
                    creature.changed = true;
                }
            }
        }
        for creature in &mut self.creatures.unplaced {
            if same_name(&creature.owner, &canonical) {
                creature.owner.clone_from(&canonical);
            }
        }
        let positions: Vec<_> = self.factory.iter().filter_map(|(pos, _, state)| state.owner.as_ref().filter(|name| same_name(name, &canonical)).map(|_| pos)).collect();
        for pos in positions {
            self.factory.state_mut(pos).expect("known machine").owner = Some(canonical.clone());
        }
        canonical
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
            // Feet on a ladder, nobody climbs it or slides down it faster than `climb_speed`
            // (the client's `movement::step`, a jump on it included).
            let (rise, fall) = if self.on_ladder(player.position) { (tuning.climb_speed, tuning.climb_speed) } else { (tuning.jump_velocity, tuning.terminal_velocity) };
            Budgets { across: checks.speed_margin * tuning.top_ground_speed(), rise: checks.speed_margin * rise, fall: checks.speed_margin * fall }
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
    /// `Some(false)` ground (or a ladder their feet are in, `movement::on_ladder`), `None`
    /// nothing. What they may jump or swim up from. Where the server has not loaded the land
    /// yet, it takes the player's word.
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
        } else if ground.overlaps(|cell| self.boxes(cell)) || self.on_ladder(feet) {
            Some(false)
        } else {
            None
        }
    }

    /// Whether a player's feet at `feet` are in a ladder's cell, as the client climbs it
    /// (`movement::on_ladder`).
    pub(crate) fn on_ladder(&self, feet: Vec3) -> bool {
        let half = dims::WIDTH / 2.0 - 1e-4;
        let (lo, hi) = ((feet - Vec3::new(half, 0.0, half)).floor().as_ivec3(), (feet + Vec3::new(half, 0.0, half)).floor().as_ivec3());
        (lo.x..=hi.x).any(|x| (lo.z..=hi.z).any(|z| self.block(voxmar_engine::glam::IVec3::new(x, lo.y, z)).is_some_and(|b| self.content.blocks.info(b).climbable)))
    }

    /// Whether there is ground within `Checks::near_ground` under a player at `feet`.
    fn near_ground(&self, feet: Vec3) -> bool {
        let half = dims::WIDTH / 2.0;
        let below = Aabb { min: feet - Vec3::new(half, self.content.movement.checks.near_ground, half), max: feet + Vec3::new(half, 0.0, half) };
        below.overlaps(|cell| self.boxes(cell))
    }

    /// Where a player above `from` would stand on the first solid block under them, on the
    /// highest top under their footprint (a slab's at half a block; a post only where they
    /// are over it), within 64 blocks; otherwise where they are.
    fn ground_below(&self, from: Vec3) -> Vec3 {
        let half = dims::WIDTH / 2.0;
        let (lo, hi) = (from - Vec3::new(half, 0.0, half), from + Vec3::new(half, 0.0, half));
        let (a, b) = (lo.floor().as_ivec3(), hi.floor().as_ivec3());
        (0..64)
            .map(|down| from.floor().y as i32 - down)
            .find_map(|y| {
                let mut top = None::<f32>;
                for x in a.x..=b.x {
                    for z in a.z..=b.z {
                        let cell = voxmar_engine::glam::IVec3::new(x, y, z);
                        for b in self.boxes(cell) {
                            let (min, max) = (b.min + cell.as_vec3(), b.max + cell.as_vec3());
                            if min.x < hi.x && max.x > lo.x && min.z < hi.z && max.z > lo.z {
                                top = Some(top.map_or(max.y, |t: f32| t.max(max.y)));
                            }
                        }
                    }
                }
                let on = from.with_y(top?);
                (!self.inside(on)).then_some(on)
            })
            .unwrap_or(from)
    }

    /// Whether a player at `feet` would be in a wall, with a slightly smaller box than theirs
    /// for rounding: a share of it against whole blocks (`Checks::wall_box`), their own less
    /// a fixed margin against shaped blocks' boxes (`Checks::wall_margin`), so a gap between
    /// shapes narrower than the player is no way through.
    fn inside(&self, feet: Vec3) -> bool {
        let checks = &self.content.movement.checks;
        let (width, height) = checks.wall_box;
        let m = checks.wall_margin;
        let half = dims::WIDTH / 2.0 - m;
        let tight = Aabb { min: feet - Vec3::new(half, -m, half), max: feet + Vec3::new(half, dims::HEIGHT - m, half) };
        // Near a shape every block is judged by the margin: a whole block's share and a
        // shape's margin together would leave a gap narrower than the player open. Near is
        // a shape in a cell the margin box itself reaches into, past rounding: a shape can
        // only come or go in a cell the player's own box at most touches (one placed is
        // refused in a cell they are in), and the share box sits inside the margin box, so
        // which box judges a spot never flips it from clear to inside.
        let near_shape = tight.intersects(|cell| {
            let boxes = self.boxes(cell);
            !boxes.is_empty() && boxes != CUBE
        });
        if near_shape {
            tight.overlaps(|cell| self.boxes(cell))
        } else {
            Aabb::standing(feet + Vec3::Y * m, dims::WIDTH * width, dims::HEIGHT * height).overlaps(|cell| self.boxes(cell))
        }
    }

    /// Whether a player could have got from `from` to `to` without passing through a wall.
    /// Between two inputs the client moved them along some curve: off a ledge they stay
    /// high and then drop, onto a step they rise and then go on, so the straight line can
    /// cut a corner they went round. A way is clear if the straight line is, or the path
    /// across and then up or down, or up or down and then across (the client moves along
    /// one axis at a time). A player the world has closed in on (a block placed on them)
    /// is judged by where they end up only, so they can always step out.
    fn clear_between(&self, from: Vec3, to: Vec3) -> bool {
        // Standing still goes into nothing, even in a wall or in land not loaded yet.
        if from == to {
            return true;
        }
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
        clear(from, to) || (clear(from, across_first) && clear(across_first, to)) || (clear(from, up_first) && clear(up_first, to)) || self.stepped_between(from, to)
    }

    /// Whether a walker could have got from `from` to `to` along the ground, stepping up onto
    /// each block in the way no taller than `step_up` as the client does: between two inputs
    /// a sprint at a low frame rate climbs a slab and then a stair under a ceiling that no
    /// straight or one-turn path clears.
    fn stepped_between(&self, from: Vec3, to: Vec3) -> bool {
        let tuning = &self.content.movement;
        // As many steps as the rise budget lets one input climb (about a block and a half):
        // each is checked below.
        if to.y < from.y {
            return false;
        }
        let across = (to - from).with_y(0.0);
        let steps = (across.length() / tuning.checks.sweep_step).ceil().max(1.0) as u32;
        let mut at = from;
        for i in 1..=steps {
            let mut next = (from + across * (i as f32 / steps as f32)).with_y(at.y);
            if self.inside(next) {
                // Up onto what is in the way, from where the walker was: its top, no more
                // than a step up.
                let Some(top) = self.top_within(next, tuning.step_up) else { return false };
                next.y = top;
                if self.inside(at.with_y(top)) || self.inside(next) {
                    return false;
                }
            }
            at = next;
        }
        // Then down, if the last step overshot, to where they are: through nothing on the way
        // (a roof walked up onto is no way into the room under it).
        if at.y + 1e-3 < to.y {
            return false;
        }
        let drop = ((at.y - to.y) / tuning.checks.sweep_step).ceil().max(1.0) as u32;
        (1..=drop).all(|i| !self.inside(to.with_y(at.y + (to.y - at.y) * i as f32 / drop as f32)))
    }

    /// The highest top of a block's boxes under a player's footprint at `feet` that is above
    /// their feet but at most `rise` above.
    fn top_within(&self, feet: Vec3, rise: f32) -> Option<f32> {
        let half = dims::WIDTH / 2.0;
        let (lo, hi) = (feet - Vec3::new(half, 0.0, half), feet + Vec3::new(half, rise, half));
        let (a, b) = (lo.floor().as_ivec3(), hi.floor().as_ivec3());
        let mut top = None::<f32>;
        for x in a.x..=b.x {
            for y in a.y..=b.y {
                for z in a.z..=b.z {
                    let cell = voxmar_engine::glam::IVec3::new(x, y, z);
                    for b in self.boxes(cell) {
                        let (min, max) = (b.min + cell.as_vec3(), b.max + cell.as_vec3());
                        let over = min.x < hi.x && max.x > lo.x && min.z < hi.z && max.z > lo.z;
                        if over && max.y > feet.y && max.y <= feet.y + rise {
                            top = Some(top.map_or(max.y, |t| t.max(max.y)));
                        }
                    }
                }
            }
        }
        top
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

/// A player's draw key: the world's secret one mixed with the name their file is kept
/// under, the same each time they join and never another player's.
pub(crate) fn player_draw_key(world: u64, name: &str) -> u64 {
    crate::player_key(name).bytes().fold(world ^ 0x5d5e_c7a1_9b3f_0e27, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// Whether two names are one player's: names differ in case only for show.
pub(crate) fn same_name(a: &str, b: &str) -> bool {
    crate::player_key(a) == crate::player_key(b)
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

#[cfg(test)]
mod tests {
    use super::*;
    use voxmar_engine::dropped::Loose;
    use voxmar_protocol::{ClientConnection, local_pair};
    use crate::items::{Items, owner_of};
    use crate::save::WorldSave;

    /// The same case as the client's `an_unloaded_cell_stops_a_player_as_a_whole_cube`: a
    /// cell in a loaded column of air stops nothing; one in the next column, not loaded, and
    /// one under the world stop a player as a whole cube, so the server's move checks agree
    /// with the client's prediction at the edge of the loaded world. Before: the server took
    /// them for empty, so a player the client held at the edge could walk or fall on there.
    #[test]
    fn an_unloaded_cell_stops_a_player_as_a_whole_cube() {
        use voxmar_engine::glam::IVec3;
        let mut server = crate::test_server();
        // Far from spawn, where nothing is loaded.
        let base = IVec3::new(1 << 16, 0, 1 << 16);
        let loaded = base + IVec3::new(1, 64, 1);
        server.world.insert(ColumnPos::of_block(loaded), std::sync::Arc::new(voxmar_engine::world::Column::default()));
        assert!(!server.world.contains(ColumnPos::of_block(loaded + IVec3::X * voxmar_engine::coords::CHUNK_SIZE)), "the fixture means the next column unloaded");
        assert!(server.boxes(loaded).is_empty());
        for unloaded in [loaded + IVec3::X * voxmar_engine::coords::CHUNK_SIZE, IVec3::new(loaded.x, -1, loaded.z)] {
            assert_eq!(server.boxes(unloaded), &voxmar_engine::physics::CUBE, "{unloaded}");
        }
        // A player at the edge, stepping over it, would be in a wall there.
        let size = voxmar_engine::coords::CHUNK_SIZE;
        let edge = Vec3::new((loaded.x - loaded.x.rem_euclid(size) + size) as f32 - 0.1, loaded.y as f32, loaded.z as f32 + 0.5);
        assert!(server.inside(edge + Vec3::X * 0.5) && !server.inside(edge - Vec3::X * 0.5));
    }

    /// A player standing still where their own column is not loaded yet (at login, after a
    /// long teleport) is in a whole cube there; standing still is no move into anything.
    /// Before: each input was refused and sent a teleport back to where they stood.
    #[test]
    fn a_player_standing_still_in_land_not_loaded_is_not_teleported() {
        let mut server = crate::test_server();
        let _connection = join(&mut server, 1, "Ada");
        let far = Vec3::new((1 << 16) as f32 + 0.5, 70.0, (1 << 16) as f32 + 0.5);
        assert!(!server.world.contains(ColumnPos::of_block(far.floor().as_ivec3())), "the fixture means their column unloaded");
        let player = server.players.get_mut(&1).unwrap();
        (player.position, player.accepted) = (far, far);
        let before = player.teleports;
        for _ in 0..20 {
            let teleport = server.players[&1].teleports;
            server.input(1, PlayerInput { position: far, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false, teleport });
        }
        assert_eq!(server.players[&1].teleports, before, "standing still was refused");
        assert_eq!(server.players[&1].position, far);
    }

    /// Up a ladder thirty blocks high, or down it, a player sending where they are twenty
    /// times a second is taken at the ladder's own pace (3 blocks a second), and put back
    /// at more than twice that, though a jump goes faster and a fall faster still: on a
    /// ladder nobody rises or sinks faster than `climb_speed` and the margin allow, once
    /// the moves they have in hand run out.
    #[test]
    fn a_climb_or_slide_faster_than_a_ladder_allows_is_put_back() {
        use voxmar_engine::glam::IVec3;
        let mut server = crate::test_server();
        let _connection = join(&mut server, 1, "Ada");
        let at = server.spawn.floor().as_ivec3() + IVec3::new(0, 20, 0);
        server.load_area(at - IVec3::splat(4), at + IVec3::new(4, 36, 4));
        let blocks = &server.content.blocks;
        let (stone, planks, ladder) = (blocks.expect("base:stone"), blocks.expect("base:oak_planks"), blocks.expect("base:oak_ladder"));
        for y in 0..=32 {
            for x in -1..=1 {
                for z in -1..=1 {
                    server.set_block(at + IVec3::new(x, y, z), voxmar_engine::block::BlockId::AIR);
                }
                server.set_block(at + IVec3::new(x, y, 1), planks);
            }
            server.set_block(at + IVec3::Y * y, ladder);
        }
        server.set_block(at - IVec3::Y, stone);
        let bottom = at.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
        // From `from`, `steps` inputs a twentieth of a second apart, each `by` further up;
        // whether any was put back.
        let mut clock = Instant::now();
        let mut go = |server: &mut Server, from: Vec3, by: f32, steps: usize| -> bool {
            let player = server.players.get_mut(&1).unwrap();
            (player.position, player.accepted, player.budget_at) = (from, from, None);
            let before = player.teleports;
            for k in 1..=steps {
                clock += std::time::Duration::from_millis(50);
                server.arrival = clock;
                let teleport = server.players[&1].teleports;
                server.input(1, PlayerInput { position: from + Vec3::Y * by * k as f32, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false, teleport });
            }
            clock += std::time::Duration::from_secs(5);
            server.players[&1].teleports != before
        };
        let top = bottom + Vec3::Y * 30.0;
        assert!(!go(&mut server, bottom, 0.15, 200), "an honest climb was put back");
        assert!(!go(&mut server, top, -0.15, 200), "an honest slide was put back");
        assert!(go(&mut server, bottom, 0.4, 75), "a climb at 8 blocks a second was let through");
        assert!(go(&mut server, top, -0.4, 75), "a slide at 8 blocks a second was let through");
    }

    fn join(server: &mut Server, id: PlayerId, name: &str) -> ClientConnection {
        let (connection, server_side) = local_pair();
        let player = Player::new(id, server_side, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        server.players.insert(id, player);
        server.hello(id, name.into(), PROTOCOL_VERSION, server.content.fingerprint(), None);
        connection
    }

    /// Joins from another place than the in-process host, over a real socket: a second
    /// connection in process is the host coming back, which replaces the first.
    fn join_from_elsewhere(server: &mut Server, id: PlayerId, name: &str) -> ClientConnection {
        let (accepted, incoming) = crossbeam_channel::unbounded();
        let addr = voxmar_protocol::tcp::listen("127.0.0.1:0", accepted).unwrap();
        let connection = voxmar_protocol::tcp::connect(addr).unwrap();
        let server_side = incoming.recv_timeout(std::time::Duration::from_secs(30)).expect("the connection is accepted");
        let player = Player::new(id, server_side, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        server.players.insert(id, player);
        server.hello(id, name.into(), PROTOCOL_VERSION, server.content.fingerprint(), None);
        connection
    }

    /// Every Players message a client was sent since last asked: (seen, gone) each.
    fn told_of_others(client: &ClientConnection) -> Vec<(Vec<PlayerView>, Vec<PlayerId>)> {
        told_of_others_and_names(client).into_iter().map(|(seen, gone, _)| (seen, gone)).collect()
    }

    /// Every Players message a client was sent since last asked, names out of sight and all.
    #[allow(clippy::type_complexity)]
    fn told_of_others_and_names(client: &ClientConnection) -> Vec<(Vec<PlayerView>, Vec<PlayerId>, Vec<(PlayerId, String)>)> {
        std::iter::from_fn(|| client.try_recv().ok().flatten())
            .filter_map(|message| match message {
                ServerMessage::Players { seen, gone, names } => Some((seen, gone, names)),
                _ => None,
            })
            .collect()
    }

    /// Two players joined, Bob holding an iron sword: Ada is told of him by name, with what
    /// he holds.
    fn ada_and_bob() -> (Server, ClientConnection, ClientConnection, BlockId) {
        let mut server = crate::test_server();
        let ada = join(&mut server, 1, "Ada");
        let bob = join(&mut server, 2, "Bob");
        let sword = server.content.blocks.id("base:iron_sword").expect("a sword");
        let p = server.players.get_mut(&2).unwrap();
        p.inventory.add(sword, 1);
        p.selected = (0..voxmar_engine::inventory::SLOTS).find(|&s| p.inventory.get(s).is_some_and(|stack| stack.item == sword)).expect("the sword in a slot");
        (server, ada, bob, sword)
    }

    /// Bob moved by his own input, looking `pitch` up.
    fn bob_moves(server: &mut Server, bob: &ClientConnection, by: Vec3, pitch: f32) {
        let p = &server.players[&2];
        let input = PlayerInput { position: p.position + by, yaw: 0.3, pitch, selected_slot: p.selected as u8, flying: false, teleport: p.teleports };
        bob.send(ClientMessage::Input(input));
        server.handle_messages();
    }

    /// Others see what a player holds and how they look up or down (at most as far as the
    /// server lets them: 1.6 radians); the last one left is told the others are gone, once.
    #[test]
    fn others_are_told_what_a_player_holds_and_where_they_look_and_when_they_go() {
        let (mut server, ada, bob, sword) = ada_and_bob();
        bob_moves(&mut server, &bob, Vec3::ZERO, 3.0);
        server.broadcast_players();
        let told = told_of_others(&ada);
        let (seen, _) = told.last().expect("Ada was told of Bob");
        let view = seen.iter().find(|v| v.id == 2).expect("Bob among the others");
        assert_eq!((view.held, view.pitch, view.name.as_deref()), (Some(sword), 1.6, Some("Bob")), "what Bob holds, how far up he looks and his name");
        assert!(seen.iter().all(|v| v.id != 1), "Ada was told of herself");
        server.leave(2);
        server.broadcast_players();
        let told = told_of_others_and_names(&ada);
        assert!(
            told.len() == 1 && told[0].0.is_empty() && told[0].1 == [2] && told[0].2 == [(2, String::new())],
            "Ada was not told once that Bob had gone, his name to forget: {told:?}"
        );
        server.broadcast_players();
        assert!(told_of_others(&ada).is_empty(), "alone, Ada was told of nobody again and again");
    }

    /// A view unchanged since it was sent is not sent again, and a name only the first time.
    #[test]
    fn an_unchanged_player_is_not_told_again_and_a_name_is_told_once() {
        let (mut server, ada, bob, _) = ada_and_bob();
        server.broadcast_players();
        assert_eq!(told_of_others(&ada).len(), 1, "Ada was not told of Bob");
        for _ in 0..3 {
            server.broadcast_players();
        }
        assert!(told_of_others(&ada).is_empty(), "Bob standing still was told again");
        bob_moves(&mut server, &bob, Vec3::new(0.2, 0.0, 0.0), 0.0);
        server.broadcast_players();
        let told = told_of_others(&ada);
        assert_eq!(told.len(), 1, "Bob moving was not told");
        let view = &told[0].0[0];
        assert!(view.id == 2 && view.name.is_none(), "Bob's name was told again: {view:?}");
    }

    /// A player further than `players_seen` is not told of, only named, once (for a
    /// creature they keep); coming near, they are told of, the name not again.
    #[test]
    fn a_far_player_is_not_told_of() {
        let (mut server, ada, _bob, _) = ada_and_bob();
        let seen = server.config.limits.players_seen;
        let far = server.players[&1].position + Vec3::new(seen + 10.0, 0.0, 0.0);
        server.players.get_mut(&2).unwrap().position = far;
        server.broadcast_players();
        let told = told_of_others_and_names(&ada);
        assert!(
            told.len() == 1 && told[0].0.is_empty() && told[0].2 == [(2, "Bob".to_owned())],
            "Ada was told of Bob {} blocks off, or not of his name: {told:?}",
            seen + 10.0
        );
        server.broadcast_players();
        assert!(told_of_others(&ada).is_empty(), "Bob's name was told again");
        let near = server.players[&1].position + Vec3::new(seen - 10.0, 0.0, 0.0);
        server.players.get_mut(&2).unwrap().position = near;
        server.broadcast_players();
        let told = told_of_others(&ada);
        assert!(told.len() == 1 && told[0].0[0].id == 2 && told[0].0[0].name.is_none(), "Ada was not told of Bob as he came near, or his name again: {told:?}");
        server.players.get_mut(&2).unwrap().position = far;
        server.broadcast_players();
        let told = told_of_others(&ada);
        assert!(told.len() == 1 && told[0].1 == [2], "Ada was not told Bob went out of sight: {told:?}");
    }

    /// A player told of, hovering just past `players_seen`, stays told of until they go
    /// past `players_seen_margin` more: never told of and gone again tick by tick.
    #[test]
    fn a_player_at_the_edge_of_sight_stays_told_of() {
        let (mut server, ada, _bob, _) = ada_and_bob();
        let (seen, margin) = (server.config.limits.players_seen, server.config.limits.players_seen_margin);
        assert!(margin > 1.0, "the margin is some blocks");
        let at = |server: &Server, x: f32| server.players[&1].position + Vec3::new(x, 0.0, 0.0);
        let to = at(&server, seen - 0.5);
        server.players.get_mut(&2).unwrap().position = to;
        server.broadcast_players();
        assert_eq!(told_of_others(&ada).len(), 1, "Ada was not told of Bob just inside sight");
        for (tick, x) in [seen + 0.5, seen - 0.5, seen + margin - 0.5, seen - 0.25].into_iter().enumerate() {
            let to = at(&server, x);
            server.players.get_mut(&2).unwrap().position = to;
            server.broadcast_players();
            let told = told_of_others(&ada);
            assert!(told.iter().all(|t| t.1.is_empty()), "Ada was told Bob went out of sight {x} blocks off (move {tick}): {told:?}");
        }
        let to = at(&server, seen + margin + 0.5);
        server.players.get_mut(&2).unwrap().position = to;
        server.broadcast_players();
        let told = told_of_others(&ada);
        assert!(told.len() == 1 && told[0].1 == [2], "Ada was not told Bob went out of sight past the margin: {told:?}");
        // Gone, he is told of again only once back inside `players_seen`.
        let to = at(&server, seen + margin - 0.5);
        server.players.get_mut(&2).unwrap().position = to;
        server.broadcast_players();
        assert!(told_of_others(&ada).is_empty(), "Ada was told of Bob again inside the margin, not inside sight");
    }

    /// Drive the server directly: no tick race can hide an identity change or a cap bypass.
    #[test]
    fn the_saved_name_keeps_identity_herd_caps_births_and_death_drops() {
        check_saved_ownership(false);
    }

    #[test]
    fn a_legacy_name_recovers_ownership_and_folds_old_case_aliases() {
        check_saved_ownership(true);
    }

    fn check_saved_ownership(legacy: bool) {
        let mut server = crate::test_server();
        let dir = std::env::temp_dir().join(format!("voxmar-canonical-name-{legacy}-{}", std::process::id()));
        let save = WorldSave::new(&dir);
        save.create(&server.meta).unwrap();
        server.save = Some(save.clone());
        let original = join(&mut server, 1, "Bob");
        let hen = server.content.creatures.find("junglefowl").unwrap();
        let parents: Vec<_> = (0..2).map(|salt| server.spawn_creature(hen, server.spawn + Vec3::X * 3.0, salt).unwrap()).collect();
        for &id in &parents {
            let creature = server.creatures.live.get_mut(&id).unwrap();
            creature.owner = Some("Bob".into());
            creature.fed = -1.0e6;
        }
        // A previous case-changing login could already have left aliases in the world.
        server.creatures.live.get_mut(&parents[1]).unwrap().owner = Some("bOb".into());
        let wheel = server.content.blocks.expect("base:treadwheel");
        let wheel_pos = (server.spawn + Vec3::X * 5.0).floor().as_ivec3();
        assert!(server.factory.insert(wheel_pos, wheel, &server.content.machines));
        server.factory.state_mut(wheel_pos).unwrap().owner = Some("bOb".into());
        // Both live animals and those waiting in the save count towards the keeper's cap.
        let kept_each = server.content.creatures.limits().kept_each;
        let waiting = server.saved_creatures()[0].clone();
        server.creatures.unplaced = vec![waiting; kept_each - parents.len()];
        let dirt = server.content.blocks.expect("base:dirt");
        server.items.spawn_for(ItemStack::new(dirt, 7), Loose::new(server.spawn + Vec3::Y * 0.8, Vec3::ZERO), 0.0, Some(owner_of("Bob")), 10_000);
        server.items.spawn_for(ItemStack::new(dirt, 2), Loose::new(server.spawn + Vec3::Y * 0.8, Vec3::ZERO), 0.0, Some(owner_of("bOb")), 10_000);
        server.items.spawn_for(ItemStack::new(dirt, 3), Loose::new(server.spawn + Vec3::Y * 0.8, Vec3::ZERO), 0.0, Some(owner_of("someone-else")), 10_000);
        server.leave(1);
        drop(original);
        if legacy {
            let path = dir.join("players/bob.ron");
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains("name: Some(\"Bob\"),"));
            std::fs::write(path, text.replace("name: Some(\"Bob\"),", "")).unwrap();
            assert_eq!(save.saved_name("BOB"), None);
        } else {
            assert_eq!(save.saved_name("BOB").as_deref(), Some("Bob"));
        }
        // Read the persisted drops back, so their owner hash must survive saving too.
        server.items = Items::restore(save.read_items().unwrap(), |name| server.content.blocks.item_named(name));
        let returning = join(&mut server, 2, "BOB");
        assert_eq!(server.players[&2].name, "Bob");
        assert_eq!(server.players[&2].view().name.as_deref(), Some("Bob"), "replicated identity uses the saved name");
        assert_eq!(server.factory.state_mut(wheel_pos).unwrap().owner.as_deref(), Some("Bob"));
        assert_eq!(server.kept_by(&server.players[&2].name), kept_each);
        assert_eq!(server.birth_room(&server.players[&2].name), 0, "case changes cannot evade the herd cap");
        let wheat = server.content.blocks.expect("base:wheat");
        assert!(server.creature_accepts(2, parents[0], wheat), "their hungry animal still accepts food");
        server.tick_husbandry();
        assert_eq!(server.creatures.live[&parents[0]].owner_id, Some(2));
        // Duplicate rejection remains case-insensitive after canonicalising the session.
        let duplicate = join_from_elsewhere(&mut server, 3, "bOb");
        assert!(!server.players[&3].joined);
        let mut told = None;
        for _ in 0..30_000 {
            told = duplicate.try_recv().unwrap();
            if told.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(matches!(told, Some(ServerMessage::Rejected { .. })), "the duplicate was not turned away: {told:?}");
        server.tick_items();
        let picked: u32 = server.players[&2].inventory.slots.iter().flatten().filter(|s| s.item == dirt).map(|s| s.count as u32).sum();
        assert_eq!(picked, 9, "both known spellings of their saved death drops are collectible");
        let left = server.items.saved(|item| server.content.blocks.def(item).name.clone());
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].owner, Some(owner_of("someone-else")), "an unknown hash stays protected");
        // Once there is room, a birth credits the returning keeper and retains ownership.
        server.creatures.unplaced.clear();
        let farming = server.content.skills.id("core:farming").unwrap();
        let before = server.players[&2].skills.xp(farming);
        for &id in &parents {
            server.creatures.live.get_mut(&id).unwrap().ready_until = server.tick + 100;
        }
        server.tick_husbandry();
        assert!(server.creatures.live.len() > parents.len());
        assert!(server.creatures.live.values().all(|c| c.owner.as_deref() == Some("Bob")));
        assert!(server.players[&2].skills.xp(farming) > before, "birth experience goes to the returning keeper");
        drop(returning);
        drop(server);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A player joining while thatch smoulders is told where at once, though the others
    /// were told as it began and nothing has changed since.
    #[test]
    fn a_player_joining_is_told_what_smoulders() {
        use voxmar_engine::glam::IVec3;
        let mut server = crate::test_server();
        let at = voxmar_engine::coords::ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 10, 8);
        server.set_block(at - IVec3::Y, server.content.blocks.expect("base:stone"));
        server.set_block(at + IVec3::X - IVec3::Y, server.content.blocks.expect("base:stone"));
        server.set_block(at, server.content.blocks.expect("base:wattle_panel"));
        server.set_block(at + IVec3::X, server.content.blocks.expect("base:torch"));
        server.send_smoulders();
        let client = join(&mut server, 2, "Late");
        let told: Vec<Vec<IVec3>> = std::iter::from_fn(|| client.try_recv().ok().flatten()).filter_map(|m| match m { ServerMessage::Smoulders { at } => Some(at), _ => None }).collect();
        assert_eq!(told, [vec![at]]);
    }

    /// What stands on a mantel shelf is told with its column, and as it changes to the
    /// players who have the column, only them: the item in each of its slots, in order, a
    /// spot apiece. A shelf turned east keeps things as one facing north does.
    #[test]
    fn what_stands_on_a_shelf_is_told_to_whoever_has_its_column() {
        use voxmar_engine::coords::ColumnPos;
        use voxmar_engine::glam::IVec3;
        use voxmar_engine::inventory::ItemStack;
        type Told = Vec<(ColumnPos, Vec<(IVec3, Vec<Option<BlockId>>)>)>;
        let mut server = crate::test_server();
        let at = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 10, 8);
        let column = ColumnPos::of_block(at);
        let (shelf, stick) = (server.content.blocks.expect("base:oak_mantel_shelf_east"), server.content.blocks.expect("base:stick"));
        server.set_block(at, shelf);
        let machines = server.content.machines.clone();
        assert!(server.factory.insert(at, shelf, &machines), "a shelf turned east is a store");
        let spots = server.content.blocks.def(shelf).shows.len();
        server.factory.state_mut(at).unwrap().bin[1] = Some(ItemStack::new(stick, 1));
        let shown = |client: &ClientConnection| -> Told {
            std::iter::from_fn(|| client.try_recv().ok().flatten()).filter_map(|m| match m { ServerMessage::Shown { column, at } => Some((column, at)), _ => None }).collect()
        };
        let near = join(&mut server, 2, "Near");
        let far = join(&mut server, 3, "Far");
        server.players.get_mut(&3).unwrap().sent.clear();
        server.send_shown();
        server.stream_columns(2);
        let mut things = vec![None; spots];
        things[1] = Some(stick);
        assert_eq!(shown(&near), [(column, vec![(at, things)])], "told once, with the column or as it changed");
        // Taken off, it is told gone, once, and only to whoever has the column.
        server.factory.state_mut(at).unwrap().bin[1] = None;
        server.send_shown();
        server.send_shown();
        assert_eq!(shown(&near), [(column, Vec::new())]);
        assert_eq!(shown(&far), Told::new(), "a player without the column is told nothing of it");
    }

    /// A full shelf broken drops its things, and whoever has its column is told it shows
    /// nothing there now.
    #[test]
    fn a_full_shelf_broken_is_told_empty_to_whoever_has_its_column() {
        use voxmar_engine::coords::ColumnPos;
        use voxmar_engine::glam::IVec3;
        use voxmar_engine::inventory::ItemStack;
        type Told = Vec<(ColumnPos, Vec<(IVec3, Vec<Option<BlockId>>)>)>;
        let mut server = crate::test_server();
        let at = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 10, 8);
        let column = ColumnPos::of_block(at);
        let (shelf, stick) = (server.content.blocks.expect("base:oak_mantel_shelf"), server.content.blocks.expect("base:stick"));
        server.set_block(at, shelf);
        let spots = server.content.blocks.def(shelf).shows.len();
        let state = server.factory.state_mut(at).expect("a shelf is a store");
        for slot in state.bin.iter_mut().take(spots) {
            *slot = Some(ItemStack::new(stick, 1));
        }
        let shown = |client: &ClientConnection| -> Told {
            std::iter::from_fn(|| client.try_recv().ok().flatten()).filter_map(|m| match m { ServerMessage::Shown { column, at } => Some((column, at)), _ => None }).collect()
        };
        let near = join(&mut server, 2, "Near");
        server.send_shown();
        server.stream_columns(2);
        assert_eq!(shown(&near), [(column, vec![(at, vec![Some(stick); spots])])], "full, every spot shown");
        server.set_block(at, BlockId::AIR);
        server.send_shown();
        assert_eq!(shown(&near), [(column, Vec::new())], "broken, its column shows nothing");
    }

    /// A player let down lands on a post's top only where they are over it: beside it, in
    /// the empty part of its cell, they go on down to the floor under it.
    #[test]
    fn a_let_down_lands_on_a_post_only_where_it_is_under_the_player() {
        let mut server = crate::test_server();
        let at = server.spawn.floor().as_ivec3() + voxmar_engine::glam::IVec3::new(0, 20, 0);
        server.load_area(at - voxmar_engine::glam::IVec3::splat(4), at + voxmar_engine::glam::IVec3::splat(4));
        let air = voxmar_engine::block::BlockId::AIR;
        let (stone, post) = (server.content.blocks.id("base:stone").unwrap(), server.content.blocks.id("base:oak_post").unwrap());
        for y in -3..=2 {
            for z in -2..=2 {
                for x in -2..=2 {
                    let cell = at + voxmar_engine::glam::IVec3::new(x, y, z);
                    server.set_block(cell, if y == -3 { stone } else { air });
                }
            }
        }
        server.set_block(at - voxmar_engine::glam::IVec3::Y, post);
        let cell = at.as_vec3();
        // Over the post: on its top.
        assert_eq!(server.ground_below(cell + Vec3::new(0.5, 1.5, 0.5)).y, cell.y);
        // Beside it in its cell, the footprint clear of it: down to the stone.
        let beside = cell + Vec3::new(0.99, 1.5, 0.99);
        assert_eq!(server.ground_below(beside), beside.with_y(cell.y - 2.0), "stood on a post's top beside it");
    }
}
