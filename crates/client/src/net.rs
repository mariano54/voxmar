//! Connection to the (local or remote) server: applies replicated state and sends the
//! player's input and actions. Present while in game.

use bevy::prelude::*;
use rustc_hash::FxHashMap;
use voxmar_protocol::{
    ClientConnection, ClientMessage, PlayerId, PlayerInput, PlayerView, ServerMessage, TICKS_PER_SECOND,
};
use voxmar_engine::action::{ActionId, ActionTarget};
use voxmar_engine::block::{BlockId, RenderKind};
use voxmar_server::ServerHandle;

use crate::lang::Locale;
use crate::drops::Drops;
use crate::hud::skills::SkillToasts;
use crate::session::Launch;
use crate::breaking::{BlockBroken, SHOWN_WITHIN, shows_breaking};
use crate::sfx::{SoundCue, station_cue};
use crate::hud::console::Console;
use crate::menu::{Notice, Screen};
use crate::player::LocalPlayer;
use crate::render::DayCycle;
use crate::render::lod::WorldGenerator;
use crate::weather::ClientWeather;
use crate::weather::atmosphere::ClientAtmosphere;
use crate::world::ClientWorld;

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        let in_game = in_state(Screen::InGame);
        app.init_resource::<RemotePlayers>()
            .init_resource::<Clock>()
            .add_systems(PreUpdate, receive.run_if(in_game.clone()))
            .add_systems(Update, advance_day.run_if(in_game.clone()))
            .add_systems(PostUpdate, send_input.run_if(in_game));
    }
}

/// An action the player proposed, and what it was done with, to be heard by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposed {
    pub action: ActionId,
    pub with: Vec<BlockId>,
}

#[derive(Resource)]
pub struct Net {
    // Declared first so it closes before the server stops.
    connection: ClientConnection,
    /// The integrated server, when playing singleplayer. Dropping it stops the server,
    /// which saves the world.
    server: Option<ServerHandle>,
    input_timer: f32,
    /// Actions proposed since `sfx` last took them, to be heard as they are done.
    proposed: std::sync::Mutex<Vec<Proposed>>,
    /// Actions proposed since `fx` last took them, with what they aim at, to be seen.
    shown: std::sync::Mutex<Vec<(ActionId, ActionTarget)>>,
    /// The integrated server's ticks as `receive` began: every message of those ticks had
    /// been sent, and was taken in (`taken_ticks`).
    taken: std::sync::atomic::AtomicU64,
}

impl Net {
    /// Says hello over `connection`, giving the admin token kept for `address` (the
    /// server's `host:port`, none in singleplayer) if there is one.
    pub fn new(connection: ClientConnection, server: Option<ServerHandle>, name: String, content: u64, address: Option<&str>) -> Self {
        let tokens = std::env::var("VOXMAR_ADMIN_TOKENS").unwrap_or_default();
        let admin_token = address.and_then(|address| admin_token_for(address, &tokens));
        connection.send(ClientMessage::Hello { name, protocol: voxmar_protocol::PROTOCOL_VERSION, content, admin_token });
        Self { connection, server, input_timer: 0.0, proposed: Default::default(), shown: Default::default(), taken: Default::default() }
    }

    /// The next message from the server, for tests that play without the app.
    #[cfg(test)]
    pub fn try_recv(&self) -> Result<Option<voxmar_protocol::ServerMessage>, voxmar_protocol::Disconnected> {
        self.connection.try_recv()
    }

    /// How many of the integrated server's ticks this client has taken in: every message
    /// they sent has been read (`receive`). Unlike `ServerHandle::ticks`, which runs on while
    /// a long frame draws, it never counts a tick whose news is still waiting to be read.
    pub fn taken_ticks(&self) -> Option<u64> {
        self.server.as_ref().map(|_| self.taken.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Whether this is a remote server rather than singleplayer's own.
    pub fn is_remote(&self) -> bool {
        self.server.is_none()
    }

    /// The integrated server, when playing singleplayer.
    pub fn server(&self) -> Option<&ServerHandle> {
        self.server.as_ref()
    }

    pub fn propose(&self, action: ActionId, target: ActionTarget) {
        self.propose_with(action, target, Vec::new());
    }

    /// Proposes `action`, done with `with` (the items in the crafting grid, or the block
    /// worked), which say how it sounds (`sounds.ron`'s `ways`).
    pub fn propose_with(&self, action: ActionId, target: ActionTarget, with: Vec<BlockId>) {
        self.connection.send(ClientMessage::Propose { action, target });
        self.proposed.lock().unwrap_or_else(|e| e.into_inner()).push(Proposed { action, with });
        self.shown.lock().unwrap_or_else(|e| e.into_inner()).push((action, target));
    }

    /// The actions proposed since the last call: the client proposes only what it has
    /// checked the server will allow, so each is heard as it is done (`sfx::play_actions`).
    pub fn take_proposed(&self) -> Vec<Proposed> {
        std::mem::take(&mut *self.proposed.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The actions proposed since the last call, with their targets, to be seen as they
    /// are done (`fx`).
    pub fn take_shown(&self) -> Vec<(ActionId, ActionTarget)> {
        std::mem::take(&mut *self.shown.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Proposes an action aimed into the world (mining, placing, throwing), telling the
    /// server where the player is and looks first: inputs go only twenty times a second,
    /// and the server judges reach and aim from the last it heard, which while sprinting or
    /// flying could be well behind.
    pub fn propose_from(&self, player: &LocalPlayer, action: ActionId, target: ActionTarget) {
        self.propose_from_with(player, action, target, Vec::new());
    }

    /// [`Net::propose_from`], done with `with` (see [`Net::propose_with`]).
    pub fn propose_from_with(&self, player: &LocalPlayer, action: ActionId, target: ActionTarget, with: Vec<BlockId>) {
        self.connection.send(ClientMessage::Input(input(player)));
        self.propose_with(action, target, with);
    }

    pub fn cancel(&self) {
        self.connection.send(ClientMessage::Cancel);
    }

    /// Tells the server the timed action proposed at `target` shows done here (see
    /// `ClientMessage::Finished`).
    pub fn finished(&self, target: ActionTarget) {
        self.connection.send(ClientMessage::Finished { target });
    }

    /// The player raised or lowered their guard (`fight`).
    pub fn guard(&self, up: bool) {
        self.connection.send(ClientMessage::Guard { up });
    }

    /// Answers the server's ping `id` (`ServerMessage::Ping`).
    pub fn pong(&self, id: u32) {
        self.connection.send(ClientMessage::Pong { id });
    }

    /// The player turned a prayer on or off (`fight::quick_prayers`).
    pub fn pray(&self, prayer: voxmar_engine::prayer::PrayerId, on: bool) {
        self.connection.send(ClientMessage::Pray { prayer, on });
    }

    pub fn send_settings(&self, view_distance: u8, fly_speed: f32) {
        self.connection.send(ClientMessage::Settings { view_distance, fly_speed });
    }

    /// Asks the server to run a command (`time set noon`).
    pub fn command(&self, line: &str) {
        self.connection.send(ClientMessage::Command { line: line.into() });
    }

    /// Takes `count` of `item` from creative mode's catalogue, onto the pointer (`held`) or
    /// into the inventory (see `ClientMessage::Take`).
    pub fn take(&self, item: voxmar_engine::block::BlockId, count: u8, held: bool) {
        self.connection.send(ClientMessage::Take { item, count, held });
    }

    /// Says the recipe viewer was opened on `item` (see `ClientMessage::LookedUp`).
    pub fn looked_up(&self, item: voxmar_engine::block::BlockId) {
        self.connection.send(ClientMessage::LookedUp { item });
    }

    /// Claims a finished quest's rewards (see `ClientMessage::ClaimQuest`).
    pub fn claim_quest(&self, name: &str) {
        self.connection.send(ClientMessage::ClaimQuest { name: name.into() });
    }

    /// Asks what is on the station at `block`, looked at (see `ClientMessage::LookAtStation`).
    pub fn look_at_station(&self, block: IVec3) {
        self.connection.send(ClientMessage::LookAtStation { block });
    }

    /// Says something to everyone.
    pub fn chat(&self, text: &str) {
        self.connection.send(ClientMessage::Chat { text: text.into() });
    }
}

/// The server's time of day, as last told and advanced locally since.
#[derive(Resource, Default)]
pub struct Clock {
    /// The time of day (in ticks) when last told, and how fast it runs.
    day: f64,
    day_speed: f32,
    /// When that was, in seconds since startup.
    told_at: f64,
}

impl Clock {
    /// The time of day now, in ticks (`now` is seconds since startup).
    pub fn day(&self, now: f64) -> f64 {
        self.day + (now - self.told_at) * TICKS_PER_SECOND as f64 * self.day_speed as f64
    }
}

/// Other players as the server last told them (drawn by `figures`), and any shown with no
/// server behind them (`puppet`).
#[derive(Resource, Default)]
pub struct RemotePlayers {
    players: FxHashMap<PlayerId, PlayerView>,
    /// Shown as if told of (the gate's stand-in): the server's next list, which never has
    /// them, leaves them be.
    puppets: FxHashMap<PlayerId, PlayerView>,
    /// Every other player's name as told, near or far, kept when they go out of sight: a
    /// creature kept by one far off still shows who keeps it.
    names: FxHashMap<PlayerId, String>,
}

impl RemotePlayers {
    /// Every other player here.
    pub fn views(&self) -> impl Iterator<Item = &PlayerView> {
        self.players.values().chain(self.puppets.values())
    }

    /// What the server told of others: views changed or come into sight (a name, when
    /// missing, is the one told before), players gone or out of sight, and the names of
    /// those out of sight (an empty one: they left, and their name is forgotten).
    pub fn told(&mut self, seen: Vec<PlayerView>, gone: Vec<PlayerId>, names: Vec<(PlayerId, String)>) {
        for id in gone {
            self.players.remove(&id);
        }
        for (id, name) in names {
            if name.is_empty() {
                self.names.remove(&id);
            } else {
                self.names.insert(id, name);
            }
        }
        for mut view in seen {
            match &view.name {
                Some(name) => {
                    self.names.insert(view.id, name.clone());
                }
                None => view.name = self.names.get(&view.id).cloned(),
            }
            self.players.insert(view.id, view);
        }
    }

    /// Shows another player as if the server had told of them, or moves one shown so (the
    /// gate's stand-in, as no second client plays it). They stay, standing as last shown,
    /// until forgotten (`forget`), whatever the server tells of others meanwhile.
    pub fn puppet(&mut self, view: PlayerView) {
        self.puppets.insert(view.id, view);
    }

    /// Stops showing another player.
    pub fn forget(&mut self, id: PlayerId) {
        self.players.remove(&id);
        self.puppets.remove(&id);
    }

    fn get(&self, id: PlayerId) -> Option<&PlayerView> {
        self.puppets.get(&id).or_else(|| self.players.get(&id))
    }

    /// What another player is called, near or far, once the server has told.
    pub fn name(&self, id: PlayerId) -> Option<&str> {
        self.get(id).and_then(|view| view.name.as_deref()).or_else(|| self.names.get(&id).map(String::as_str))
    }

    /// The player nearest along the look ray within `within` blocks of the eye, and how far:
    /// each where `drawn` says their body is shown (`Figures::drawn_at`), else where the
    /// server says they are.
    pub fn looked_at(&self, eye: Vec3, look: Vec3, within: f32, drawn: impl Fn(PlayerId) -> Option<Vec3>) -> Option<(PlayerId, f32)> {
        use voxmar_engine::physics::{Aabb, player};
        self.views()
            .filter_map(|view| {
                let id = view.id;
                let at = drawn(id).unwrap_or(view.position);
                Some((id, crate::creatures::ray_box(eye, look, &Aabb::standing(at, player::WIDTH, player::HEIGHT))?))
            })
            .filter(|&(_, distance)| distance <= within)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Where another player stands, if they are in sight.
    pub fn position(&self, id: PlayerId) -> Option<Vec3> {
        self.get(id).map(|view| view.position)
    }
}

/// Applies what the server sent. If it refuses us or goes away, returns to the menus with
/// the reason.
#[allow(clippy::too_many_arguments)]
pub(crate) fn receive(
    net: Res<Net>,
    mut world: ResMut<ClientWorld>,
    mut player: ResMut<LocalPlayer>,
    mut cycle: ResMut<DayCycle>,
    mut remote: ResMut<RemotePlayers>,
    mut generator: ResMut<WorldGenerator>,
    mut notice: ResMut<Notice>,
    mut next: ResMut<NextState<Screen>>,
    mut weather: ResMut<ClientWeather>,
    (time, mut clock, mut console, mut air, mut cues): (Res<Time>, ResMut<Clock>, ResMut<Console>, ResMut<ClientAtmosphere>, MessageWriter<SoundCue>),
    (mut drops, mut toasts, launch, locale, mut creatures): (ResMut<Drops>, ResMut<SkillToasts>, Res<Launch>, Res<Locale>, ResMut<crate::creatures::Creatures>),
    (mut broken, mut catches): (MessageWriter<BlockBroken>, MessageWriter<crate::sfx::Catches>),
    mut fighting: ResMut<crate::fight::Fighting>,
    mut stations: ResMut<crate::hud::target::StationSeen>,
) {
    let menu = if net.is_remote() { Screen::Join } else { Screen::Title };
    // Counted before reading: a tick's messages are all sent before it counts, so this many
    // ticks' are all read below.
    if let Some(server) = &net.server {
        net.taken.store(server.ticks(), std::sync::atomic::Ordering::Relaxed);
    }
    loop {
        let message = match net.connection.try_recv() {
            Ok(Some(message)) => message,
            Ok(None) => break,
            Err(_) => {
                let reason = if net.is_remote() { "net.lost" } else { "net.world_stopped" };
                notice.0 = Some(locale.get(reason).into());
                next.set(menu);
                break;
            }
        };
        match message {
            ServerMessage::Welcome { player: me, position, yaw, flying, allow_flight, admin, seed, weather_seed } => {
                creatures.me = Some(me);
                weather.set_seed(weather_seed);
                air.start(seed);
                generator.start(seed, &launch.content);
                player.position = position;
                player.yaw = yaw;
                player.flying = flying;
                player.allow_flight = allow_flight;
                player.admin = admin;
                player.seed = seed;
                player.joined = true;
                player.journal = Default::default();
                player.quests.clear();
            }
            ServerMessage::Rejected { reason } => {
                notice.0 = Some(locale.format("net.rejected", &[("reason", &locale.render(&reason, &world.registry))]));
                next.set(menu);
                break;
            }
            ServerMessage::Column { pos, column } => world.insert_column(pos, column),
            ServerMessage::Unload { pos } => world.remove_column(pos),
            ServerMessage::BlockChanged { pos, block } => {
                let old = world.block(pos);
                if old != Some(block) {
                    // A break the player did not predict: someone else's, or the player's own
                    // when the server's word came first, shown nearby. Plants also vanish
                    // under feet (the server's wear), which is not breaking them, so only
                    // blocks count; a player's own plants break the moment they click.
                    if let Some(old) = old
                        && block.is_air()
                        && shows_breaking(world.registry.info(old))
                        && world.registry.info(old).render != RenderKind::Plant
                        && pos.as_vec3().distance(player.position) < SHOWN_WITHIN
                    {
                        broken.write(BlockBroken { pos, block: old });
                    }
                    // A home station loaded or emptied, nearby: the stir or the pop.
                    if let Some(old) = old
                        && pos.as_vec3().distance(player.position) < SHOWN_WITHIN
                        && let Some(cue) = station_cue(&launch.content.stations, &world.registry, old, block)
                    {
                        cues.write(cue.at(pos.as_vec3() + Vec3::splat(0.5)));
                    }
                    // Thatch or wattle catching fire, nearby.
                    if crate::sfx::catches(&launch.content.fire, old, block) && pos.as_vec3().distance(player.position) < SHOWN_WITHIN {
                        catches.write(crate::sfx::Catches { at: pos.as_vec3() + Vec3::splat(0.5) });
                    }
                    world.set_block(pos, block);
                }
            }
            ServerMessage::Blocks { changes } => {
                for (pos, block) in changes {
                    let old = world.block(pos);
                    if old != Some(block) {
                        // Thatch or wattle catching fire, nearby: the server's word, never a
                        // client timer.
                        if crate::sfx::catches(&launch.content.fire, old, block) && pos.as_vec3().distance(player.position) < SHOWN_WITHIN {
                            catches.write(crate::sfx::Catches { at: pos.as_vec3() + Vec3::splat(0.5) });
                        }
                        world.set_block(pos, block);
                    }
                }
            }
            ServerMessage::PlayerState { health, inventory, skills, vitals } => {
                player.states += 1;
                player.health = health.current;
                player.max_health = health.max;
                player.vitals = vitals;
                player.vitals_at = time.elapsed_secs();
                player.inventory = inventory;
                // A level reached since the last word is announced (not on joining).
                let registry = &launch.content.skills;
                if player.skills_known && player.skills != skills {
                    for id in registry.ids() {
                        let level = registry.level(skills.xp(id));
                        if level > registry.level(player.skills.xp(id)) {
                            toasts.level_up(id, level);
                        }
                    }
                }
                player.skills = skills;
                player.skills_known = true;
                if health.alive() {
                    player.dead = false;
                }
            }
            ServerMessage::Teleport { position, seq } => {
                player.position = position;
                player.teleports = seq;
                player.velocity = Vec3::ZERO;
                // Put somewhere, a dash under way stops: a dodge the server refused, whose
                // cooldown never ran there either.
                if fighting.dash.take().is_some() {
                    fighting.dodge_ready = fighting.dodge_was;
                }
            }
            ServerMessage::Players { seen, gone, names } => remote.told(seen, gone, names),
            ServerMessage::Time { tick, day, day_speed } => {
                *clock = Clock { day: day as f64, day_speed, told_at: time.elapsed_secs_f64() };
                weather.set_tick(tick, time.elapsed_secs_f64());
                cycle.ticks = day as f64;
            }
            ServerMessage::Abilities { allow_flight, admin } => {
                player.allow_flight = allow_flight;
                player.flying &= allow_flight;
                player.admin = admin;
            }
            ServerMessage::Chat { kind, text } => {
                let text = locale.render(&text, &world.registry);
                // Logged too, so the replies to a capture's `--command`s are on record.
                debug!("chat: {text}");
                console.push(kind, &text, time.elapsed_secs())
            }
            ServerMessage::Refused { why } => {
                let why = locale.render(&why, &world.registry);
                debug!("refused: {why}");
                toasts.refused(why, time.elapsed_secs())
            }
            ServerMessage::Weather { weather: report, effects, nearby, quick } => {
                weather.receive(report, effects, nearby, quick, time.elapsed_secs())
            }
            ServerMessage::Lightning { at, seed, cloud } => weather.lightning(at, seed, at.distance(player.eye()), cloud),
            ServerMessage::Died => player.dead = true,
            ServerMessage::Journal { entries } => player.journal.merge(entries),
            ServerMessage::Quests { states } => {
                player.quests = states;
                player.quests_rev = player.quests_rev.wrapping_add(1);
            }
            ServerMessage::Items { changed, gone } => drops.receive(changed, gone, time.elapsed_secs()),
            ServerMessage::Creatures { changed, gone } => creatures.receive(changed, gone),
            ServerMessage::Struck { creature, damage, killed, blow, pips } => {
                creatures.struck(creature, damage, blow, killed, time.elapsed_secs());
                fighting.struck(creature, damage, blow, pips, killed, time.elapsed_secs_f64());
            }
            ServerMessage::Hurt { creature, damage, defended, spoiled } => {
                let combat = &launch.content.combat;
                let held = player.inventory.get(player.selected).map(|s| s.item);
                let weapon = combat.wielded(held, |skill| launch.content.skills.level(player.skills.xp(skill)));
                // Told a round trip late: only a shot drawn with a weapon a hit spoils is let
                // go, never a melee swing begun since (a draw's cooldown outlasts the trip, so no
                // new draw can have begun).
                let spoil = (spoiled && combat.spoiled_by_hits(&weapon)).then(|| (combat.timing(&weapon), combat.feel));
                fighting.hurt(creature, damage, defended, time.elapsed_secs_f64(), spoil)
            }
            ServerMessage::Ping { id } => {
                net.pong(id);
                fighting.pings += 1;
            }
            ServerMessage::Prayers { points, most, on } => fighting.prayers = crate::fight::Prayers { points, most, on },
            ServerMessage::Fires { lit } => world.fires = lit.into_iter().collect(),
            ServerMessage::Bark { creature, bark } => creatures.bark(creature, bark),
            ServerMessage::Smoulders { at } => world.smoulders = at.into_iter().collect(),
            ServerMessage::Xp { skill, tenths } => toasts.gained(skill, tenths, time.elapsed_secs()),
            ServerMessage::Station { block, load, fire } => stations.tell(block, load, fire, time.elapsed_secs_f64()),
        }
    }
}

/// Moves the sun with the server's time of day, which may run faster than usual or not at
/// all.
fn advance_day(time: Res<Time>, clock: Res<Clock>, mut cycle: ResMut<DayCycle>) {
    if clock.told_at > 0.0 {
        cycle.ticks = clock.day(time.elapsed_secs_f64());
    }
}

fn send_input(time: Res<Time>, mut net: ResMut<Net>, player: Res<LocalPlayer>) {
    net.input_timer += time.delta_secs();
    let interval = 1.0 / TICKS_PER_SECOND as f32;
    if net.input_timer < interval || !player.joined {
        return;
    }
    net.input_timer %= interval;
    net.connection.send(ClientMessage::Input(input(&player)));
}

/// The admin token `tokens` keeps for the server at `address`: `tokens` lists
/// `host:port=token` entries, separated by spaces, commas or semicolons, so a server's token
/// goes to that server only (a server's admin token is its owner's, given with
/// `VOXMAR_ADMIN_TOKENS`; see the dedicated server's `--admin-token-file`).
pub fn admin_token_for(address: &str, tokens: &str) -> Option<String> {
    tokens
        .split([' ', ',', ';', '\n'])
        .filter_map(|entry| entry.split_once('='))
        .find(|(server, _)| crate::menu::join::with_port(server.trim()).eq_ignore_ascii_case(&crate::menu::join::with_port(address.trim())))
        .map(|(_, token)| token.trim().to_owned())
        .filter(|token| !token.is_empty())
}

/// The player's state as the server is told it.
fn input(player: &LocalPlayer) -> PlayerInput {
    PlayerInput {
        position: player.position,
        yaw: player.yaw,
        pitch: player.pitch,
        selected_slot: player.selected as u8,
        flying: player.flying,
        teleport: player.teleports,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_admin_token_goes_to_its_own_server_only() {
        let tokens = "play.example.org:25665=open-sesame, 192.168.1.4:25665=lan;[::1]:4000=six";
        assert_eq!(admin_token_for("play.example.org:25665", tokens).as_deref(), Some("open-sesame"));
        assert_eq!(admin_token_for("PLAY.example.org:25665", tokens).as_deref(), Some("open-sesame"));
        assert_eq!(admin_token_for("192.168.1.4:25665", tokens).as_deref(), Some("lan"));
        assert_eq!(admin_token_for("[::1]:4000", tokens).as_deref(), Some("six"));
        assert_eq!(admin_token_for("play.example.org:25666", tokens), None, "another port is another server");
        assert_eq!(admin_token_for("evil.example.org:25665", tokens), None);
        assert_eq!(admin_token_for("play.example.org:25665", ""), None);
        // An address typed without its port is the default port, on either side.
        assert_eq!(admin_token_for("play.example.org", tokens).as_deref(), Some("open-sesame"));
        assert_eq!(admin_token_for("192.168.1.4:25665", "192.168.1.4=bare").as_deref(), Some("bare"));
        assert_eq!(admin_token_for("::1", "[::1]:25665=six").as_deref(), Some("six"));
    }

    /// A view told without a name keeps the name told before; one gone is no longer shown,
    /// though their name is kept, as is one told of only by name, far off.
    #[test]
    fn others_are_told_as_they_change() {
        let mut players = RemotePlayers::default();
        let view = |name: Option<&str>, x: f32| PlayerView { id: 9, name: name.map(str::to_owned), position: Vec3::new(x, 0.0, 0.0), yaw: 0.0, pitch: 0.0, held: None, fight: Default::default() };
        players.told(vec![view(Some("Ada"), 0.0)], vec![], vec![]);
        players.told(vec![view(None, 1.0)], vec![], vec![]);
        assert_eq!((players.name(9), players.position(9)), (Some("Ada"), Some(Vec3::X)));
        players.told(vec![], vec![9], vec![(4, "Bob".to_owned())]);
        assert_eq!(players.views().count(), 0);
        assert_eq!((players.name(9), players.name(4), players.position(4)), (Some("Ada"), Some("Bob"), None), "names kept out of sight");
        players.told(vec![view(None, 2.0)], vec![], vec![]);
        assert_eq!(players.get(9).and_then(|v| v.name.as_deref()), Some("Ada"), "back in sight, the view takes the name kept");
        players.told(vec![], vec![], vec![(4, String::new())]);
        assert_eq!(players.name(4), None, "one who left, named with no name, is forgotten");
    }

    /// A stand-in shown with no server behind it (the gate's) stays through what the server
    /// tells of the others, which never names it, until it is forgotten.
    #[test]
    fn a_puppet_stays_through_the_servers_lists_until_forgotten() {
        let mut players = RemotePlayers::default();
        let view = |id, name: &str| PlayerView { id, name: Some(name.to_owned()), position: Vec3::ZERO, yaw: 0.0, pitch: 0.0, held: None, fight: Default::default() };
        players.puppet(view(7, "Gate"));
        players.told(vec![view(9, "Ada")], vec![], vec![]);
        players.told(vec![view(9, "Ada")], vec![], vec![]);
        let mut here: Vec<_> = players.views().map(|v| v.id).collect();
        here.sort();
        assert_eq!(here, vec![7, 9], "the stand-in and the player the server told of");
        assert_eq!(players.name(7), Some("Gate"));
        players.told(Vec::new(), vec![9], Vec::new());
        assert_eq!(players.views().map(|v| v.id).collect::<Vec<_>>(), vec![7], "the server telling the others gone leaves the stand-in");
        players.forget(7);
        assert_eq!(players.views().count(), 0);
    }

    #[test]
    fn a_player_is_looked_at_within_range_and_aim() {
        let mut players = RemotePlayers::default();
        let view = PlayerView { id: 9, name: Some("Ada".to_owned()), position: Vec3::new(0.0, 0.0, -10.0), yaw: 0.0, pitch: 0.0, held: None, fight: Default::default() };
        players.players.insert(9, view);
        let eye = Vec3::new(0.0, 1.62, 0.0);
        let (who, distance) = players.looked_at(eye, Vec3::NEG_Z, 24.0, |_| None).unwrap();
        assert_eq!(who, 9);
        assert!((distance - 9.7).abs() < 1e-4, "met at their near face: {distance}");
        assert_eq!(players.looked_at(eye, Vec3::NEG_Z, 8.0, |_| None), None, "further than they are named from");
        assert_eq!(players.looked_at(eye + Vec3::X * 2.0, Vec3::NEG_Z, 24.0, |_| None), None, "looking past them");
        let drawn = |_| Some(Vec3::new(2.0, 0.0, -10.0));
        assert_eq!(players.looked_at(eye + Vec3::X * 2.0, Vec3::NEG_Z, 24.0, drawn).map(|(who, _)| who), Some(9), "looking at their body as drawn");
        assert_eq!(players.looked_at(eye, Vec3::NEG_Z, 24.0, drawn), None, "looking where the server has them, not where they are drawn");
    }
}
