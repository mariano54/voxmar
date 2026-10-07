//! Creatures on the server: animals and monsters living near players, each an agent with
//! a brain (`engine::creature::brain`) and a body (`engine::creature::motion`).
//!
//! Every tick each creature's body moves toward what its last decision asked; every four
//! ticks (staggered by id) creatures near a player think again, every twenty further out
//! (docs/creature-ai.md, "Simulation cost"). Their decisions act on the world here: a
//! strike on a player, a mouthful of grass, a blink away, a pounce, an alarm that sets kin
//! running.
//!
//! Creatures appear near players from the land itself: every two seconds the server tries
//! a spot around each player and picks a species by how well the spot suits it (biome
//! selectors, climate comfort including aether, rarity, the hour), as plants are chosen
//! (docs/creatures.md, "Where creatures live"). Monsters come only in darkness: at night
//! under the sky, or underground away from light. Wild creatures far from every player
//! go again. The regional population process that will remember how many live where is
//! the next step (docs/creatures.md, "Populations").
//!
//! Players strike creatures with `core:attack`; hits are rolled RuneScape-style from the
//! Attack level against the creature's defence and train Attack and Hitpoints. A death
//! pops the creature's loot out as dropped items, with the killer's held tool and Hunter
//! level shifting the odds (`engine::loot`).

use std::collections::BTreeMap;

use rustc_hash::{FxHashMap, FxHashSet};
use voxmar_engine::block::{BlockId, Rarity};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::creature::brain::{self, Decision, Mind, Seen, Situation};
use voxmar_engine::creature::motion::{self, Body, Drive};
use voxmar_engine::creature::{self, CreatureId, Kind, Water, mix, unit_from};
use voxmar_engine::crop::level_bonus;
use voxmar_engine::dropped;
use voxmar_engine::genes;
use voxmar_engine::glam::{IVec3, Vec2, Vec3};
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::loot::{self, Moment};
use voxmar_engine::physics::Aabb;
use voxmar_engine::skill::SkillId;
use voxmar_engine::traits;
use voxmar_protocol::{CreatureGone, CreatureView, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::items::random3;

/// Creatures think every this many ticks within `NEAR` blocks of a player, every
/// `FAR_THINK` beyond.
const NEAR_THINK: u64 = 4;
const FAR_THINK: u64 = 20;
const NEAR: f32 = 64.0;
/// Wild creatures further than this from every player go.
const DESPAWN: f32 = 128.0;
/// Players are told about creatures within this many blocks.
const SEEN_WITHIN: f32 = 96.0;
/// Ticks between spawn attempts around each player.
const SPAWN_EVERY: u64 = 2 * TICKS_PER_SECOND as u64;
/// Spawn spots lie this far from the player, in blocks.
const SPAWN_NEAR: f32 = 20.0;
const SPAWN_FAR: f32 = 56.0;
/// How far kin hear an alarm, and are counted as kin by herding.
const KIN_RANGE: f32 = 14.0;
/// How far creatures look for food.
const FOOD_RANGE: i32 = 6;
/// How far, in blocks, a blow may reach past a creature's box on the server: the client
/// aims at where it last drew the creature, which a server running behind has moved on
/// from. Plus how far the creature moves in this many seconds.
const REACH_SLACK: f32 = 1.0;
const REACH_LAG: f32 = 0.6;
/// Seconds a player must keep within `SETTLE_RADIUS` blocks of one spot before hunters
/// there leave them be (RuneScape's ten minutes in 21 × 21 squares).
const TOLERANCE: u64 = 600 * TICKS_PER_SECOND as u64;
const SETTLE_RADIUS: f32 = 10.0;
/// Light from blocks within this many blocks keeps monsters away underground.
const LIT_REACH: i32 = 5;

/// A live creature.
#[derive(Clone, Debug)]
pub struct Live {
    pub species: CreatureId,
    pub body: Body,
    pub mind: Mind,
    pub health: u16,
    pub home: Vec3,
    pub decision: Decision,
    /// Its size against its species'.
    pub scale: f32,
    pub seed: u64,
    /// The last player to hurt it, for the kill's loot and experience.
    pub hurt_by: Option<PlayerId>,
    /// The last player to land a damaging blow on it and the tick they did: its blows that
    /// miss train that player's Defence for a while (`DefenceTraining`). With two players
    /// at it, only the last hitter's is kept. Not saved.
    pub fought: Option<(PlayerId, u64)>,
    /// Seconds without getting nearer its goal, to notice being stuck.
    pub stuck: f32,
    /// The player who keeps it, by name (tamed or bred), and the id they have now.
    pub owner: Option<String>,
    pub owner_id: Option<PlayerId>,
    /// Trust in the player feeding it, toward taming (1 is tame), and who that is.
    pub trust: f32,
    pub trusting: Option<String>,
    /// Tick it stops being ready to breed, and the tick it may breed again.
    pub ready_until: u64,
    /// The tick until which it stands still, for an admin's `/creatures hold`.
    pub held_until: u64,
    /// Its genes, and what shows of them.
    pub genome: genes::Genome,
    pub traits: genes::Traits,
    pub breeds_again: u64,
    /// Tick it grows up, while young; its grown size.
    pub grows_up: Option<u64>,
    pub adult_scale: f32,
    /// Tick each of its species' products is next due.
    pub products_due: Vec<u64>,
    /// The time of day (the day clock) a player last fed it, or it came to be: a kept
    /// animal walks a treadwheel only while fed.
    pub fed: f64,
    /// The last view each player was sent, to send only changes.
    sent: Option<CreatureView>,
    pub(crate) changed: bool,
}

impl Live {
    fn view(&self, id: u32) -> CreatureView {
        CreatureView {
            id,
            species: self.species,
            position: self.body.position,
            velocity: self.body.velocity,
            yaw: self.body.yaw,
            pose: self.decision.pose,
            health: self.health,
            scale: self.scale,
            seed: self.seed as u32,
            kept: self.owner.is_some(),
            owner: self.owner_id,
            young: self.grows_up.is_some(),
            ready: self.ready_until > 0,
            trust: if self.owner.is_some() { 0 } else { (self.trust * 100.0).clamp(0.0, 100.0) as u8 },
            genes: genes::pack(&self.genome),
        }
    }

    /// Whether this view differs enough from the last one sent to send again.
    fn moved_since(&self, last: &CreatureView) -> bool {
        last.position.distance_squared(self.body.position) > 0.0025
            || (last.yaw - self.body.yaw).abs() > 0.05
            || last.pose != self.decision.pose
            || last.health != self.health
            || last.kept != self.owner.is_some()
            || last.owner != self.owner_id
            || last.young != self.grows_up.is_some()
            || last.ready != (self.ready_until > 0)
            || last.trust != (if self.owner.is_some() { 0 } else { (self.trust * 100.0).clamp(0.0, 100.0) as u8 })
            || (last.scale - self.scale).abs() > 0.01
            || (last.velocity - self.body.velocity).length_squared() > 0.04
    }
}

/// Every live creature, by id, and what each player has been told.
#[derive(Default)]
pub struct Creatures {
    /// Ordered, so creatures act in the same order every run.
    pub live: BTreeMap<u32, Live>,
    next: u32,
    seen: FxHashMap<PlayerId, FxHashSet<u32>>,
    /// Those that died or went since players were last told.
    gone: Vec<CreatureGone>,
    /// Each player's last strike tick.
    struck: FxHashMap<PlayerId, std::time::Instant>,
    /// Where each player settled and since when (RuneScape's tolerance).
    settled: FxHashMap<PlayerId, (Vec3, u64)>,
    /// Kept creatures from the save that could not be put back yet (their species is gone
    /// from the content, or the world was full): kept in the save and tried again, never
    /// lost.
    pub unplaced: Vec<crate::husbandry::SavedCreature>,
    /// Loot waiting for its body to finish falling: the tick it drops, and what.
    falling_loot: Vec<(u64, ItemStack, dropped::Loose)>,
}

impl Creatures {
    /// Takes a creature out of the world without a death (`/creatures remove`).
    pub(crate) fn take_away(&mut self, id: u32) {
        if self.live.remove(&id).is_some() {
            self.gone.push(CreatureGone { id, died: false });
        }
    }

    /// The box a blow at a creature may land in: its own, widened by what the player's
    /// view of it may lag behind (`REACH_SLACK`, `REACH_LAG`).
    pub fn reach_box(&self, id: u32) -> Option<Aabb> {
        let creature = self.live.get(&id).filter(|c| c.health > 0)?;
        let mut aabb = creature.body.aabb();
        let slack = REACH_SLACK + creature.body.velocity.length() * REACH_LAG;
        aabb.min -= Vec3::splat(slack);
        aabb.max += Vec3::splat(slack);
        Some(aabb)
    }
}

/// The world as a creature's body feels it.
struct Ground<'a>(&'a Server);

impl motion::Surroundings for Ground<'_> {
    fn solid(&self, cell: IVec3) -> bool {
        // Unloaded columns are walls, so nothing walks off the edge of the world.
        self.0.block(cell).is_none_or(|b| self.0.content.blocks.info(b).solid)
    }

    fn water(&self, cell: IVec3) -> bool {
        self.0.block(cell).is_some_and(|b| self.0.content.blocks.info(b).fluid.is_some() && self.0.content.blocks.def(b).name.contains("water"))
    }
}

impl Server {
    fn skill(&self, name: &str) -> Option<SkillId> {
        self.content.skills.id(name)
    }

    fn level(&self, player: PlayerId, skill: Option<SkillId>) -> u8 {
        let Some(skill) = skill else { return 1 };
        self.players.get(&player).map_or(1, |p| self.content.skills.level(p.skills.xp(skill)))
    }

    /// A player's combat level, RuneScape-style (see `SkillRegistry::combat_level`).
    pub(crate) fn combat_level(&self, player: PlayerId) -> u8 {
        let skills = &self.content.skills;
        self.players.get(&player).map_or_else(|| skills.combat_level(&Default::default()), |p| skills.combat_level(&p.skills))
    }

    /// Daylight now, 0..=1.
    fn daylight(&self) -> f32 {
        creature::daylight(self.day as u64)
    }

    /// Moves, thinks for, spawns and despawns creatures, then tells players.
    pub(crate) fn tick_creatures(&mut self) {
        let _span = tracing::info_span!("creatures").entered();
        self.settle_players();
        if self.tick.is_multiple_of(SPAWN_EVERY) {
            let ids: Vec<PlayerId> = self.players.values().filter(|p| p.joined).map(|p| p.id).collect();
            for (i, id) in ids.into_iter().enumerate() {
                self.spawn_near(id, i as u64);
            }
            self.despawn_far();
        }
        self.think_and_move();
        self.tick_husbandry();
        self.drop_fallen_loot();
        if self.tick.is_multiple_of(10 * TICKS_PER_SECOND as u64) {
            self.place_unplaced();
        }
        self.replicate_creatures();
    }

    /// Notes where each player has stayed, for tolerance.
    fn settle_players(&mut self) {
        let tick = self.tick;
        let players: Vec<(PlayerId, Vec3)> = self.players.values().filter(|p| p.joined).map(|p| (p.id, p.position)).collect();
        let settled = &mut self.creatures.settled;
        settled.retain(|id, _| players.iter().any(|(p, _)| p == id));
        for (id, position) in players {
            let entry = settled.entry(id).or_insert((position, tick));
            if entry.0.distance(position) > SETTLE_RADIUS {
                *entry = (position, tick);
            }
        }
    }

    /// The players a creature at `at` can sense, nearest first.
    fn seen_from(&self, at: Vec3, range: f32, tempts: &[BlockId], owner: Option<&str>) -> Vec<Seen> {
        let mut seen: Vec<Seen> = self
            .players
            .values()
            .filter(|p| p.joined && p.health.alive() && p.position.distance(at) < range)
            .map(|p| Seen {
                id: p.id,
                position: p.position,
                tempting: p.held().is_some_and(|item| tempts.contains(&item)),
                combat_level: self.combat_level(p.id),
                creative: p.allow_flight,
                tolerated: self.creatures.settled.get(&p.id).is_some_and(|&(_, since)| self.tick - since > TOLERANCE),
                friend: owner.is_some_and(|name| name == p.name),
            })
            .collect();
        seen.sort_by(|a, b| a.position.distance(at).total_cmp(&b.position.distance(at)));
        seen
    }

    fn think_and_move(&mut self) {
        let dt = 1.0 / TICKS_PER_SECOND as f32;
        let ids: Vec<u32> = self.creatures.live.keys().copied().collect();
        let daylight = self.daylight();
        let mut actions: Vec<(u32, Decision)> = Vec::new();
        for &id in &ids {
            let creature = &self.creatures.live[&id];
            let species = self.content.creatures.get(creature.species);
            let position = creature.body.position;
            if !self.world.contains(ColumnPos::of_block(position.floor().as_ivec3())) {
                continue;
            }
            let nearest = self.players.values().filter(|p| p.joined).map(|p| p.position.distance(position)).fold(f32::MAX, f32::min);
            // Each thinks at its own beat, so neighbours do not all decide at once.
            let every = if nearest < NEAR { NEAR_THINK } else { FAR_THINK } + creature.seed % 3;
            if (self.tick + id as u64).is_multiple_of(every) {
                let range = species.def.senses.sight.max(species.def.senses.hearing).max(16.0) * 1.5;
                let players = self.seen_from(position, range, &species.favourite, creature.owner.as_deref());
                let mate = self.mate_for(id);
                let kin: Vec<Vec3> = self
                    .creatures
                    .live
                    .iter()
                    .filter(|(other, c)| **other != id && c.species == creature.species && c.body.position.distance(position) < KIN_RANGE)
                    .map(|(_, c)| c.body.position)
                    .collect();
                let food = if species.eats.is_empty() { None } else { self.food_near(position, &species.eats, id) };
                let situation = Situation {
                    position,
                    home: creature.home,
                    health: creature.health as f32 / species.def.health as f32,
                    daylight,
                    players: &players,
                    kin: &kin,
                    food,
                    mate,
                    kept: creature.owner.is_some(),
                    dt: every as f32 * dt,
                    seed: mix(creature.seed, self.tick),
                };
                let creature = self.creatures.live.get_mut(&id).expect("listed");
                let decision = if self.tick < creature.held_until {
                    brain::Decision { pose: brain::Pose::Idle, ..Default::default() }
                } else {
                    brain::think(species, &mut creature.mind, &situation)
                };
                creature.decision = decision;
                creature.changed = true;
                actions.push((id, decision));
            }
            self.move_creature(id, dt);
        }
        self.push_apart();
        for (id, decision) in actions {
            self.carry_out(id, decision);
        }
    }

    /// Creatures that touch push each other apart, as Minecraft's mobs do: they may brush
    /// and bump, but they part again rather than stand inside each other. The push grows
    /// with how far they overlap and goes into their walking speed, so walls still hold.
    fn push_apart(&mut self) {
        const PUSH: f32 = 6.0;
        let mut cells: FxHashMap<(i32, i32), Vec<u32>> = FxHashMap::default();
        for (&id, c) in &self.creatures.live {
            let p = c.body.position;
            cells.entry(((p.x / 2.0).floor() as i32, (p.z / 2.0).floor() as i32)).or_default().push(id);
        }
        let tick = self.tick;
        let mut pushes: Vec<(u32, Vec3)> = Vec::new();
        for (&(cx, cz), ids) in &cells {
            for &a in ids {
                let ca = &self.creatures.live[&a];
                for dx in -1..=1 {
                    for dz in -1..=1 {
                        let Some(others) = cells.get(&(cx + dx, cz + dz)) else { continue };
                        for &b in others.iter().filter(|&&b| b > a) {
                            let cb = &self.creatures.live[&b];
                            let apart = (cb.body.position - ca.body.position).with_y(0.0);
                            // Their bodies' reach, a little inside it, so a touch is a touch.
                            let room = (ca.body.size.0 + cb.body.size.0) * 0.5 * 0.9;
                            let tall = (ca.body.position.y - cb.body.position.y).abs() < (ca.body.size.1 + cb.body.size.1) * 0.5;
                            let d = apart.length();
                            if d >= room || !tall {
                                continue;
                            }
                            let dir = if d > 1e-3 { apart / d } else { Vec3::new((a as f32).sin(), 0.0, (a as f32).cos()) };
                            let push = dir * (room - d) / room * PUSH;
                            // The heavier one moves less.
                            let (wa, wb) = (ca.body.size.0 * ca.body.size.1, cb.body.size.0 * cb.body.size.1);
                            let total = (wa + wb).max(1e-3);
                            pushes.push((a, -push * (wb / total)));
                            pushes.push((b, push * (wa / total)));
                        }
                    }
                }
            }
        }
        for (id, push) in pushes {
            if let Some(c) = self.creatures.live.get_mut(&id).filter(|c| tick >= c.held_until) {
                c.body.velocity += push * 0.05;
                c.changed = true;
            }
        }
    }

    /// The nearest block of a creature's food within reach of `at`, if any: plants it
    /// grazes, or the grass block under them.
    fn food_near(&self, at: Vec3, eats: &[BlockId], salt: u32) -> Option<IVec3> {
        let feet = at.floor().as_ivec3();
        // A few rings out, starting at a seeded side so neighbours don't all pick the same
        // tuft.
        let start = (mix(salt as u64, self.tick / 200) % 4) as i32;
        for r in 0..=FOOD_RANGE {
            for i in 0..(8 * r).max(1) {
                let (dx, dz) = ring(r, (i + start * r.max(1)) % (8 * r).max(1));
                for dy in [0, -1, 1] {
                    let cell = feet + IVec3::new(dx, dy, dz);
                    if self.block(cell).is_some_and(|b| eats.contains(&b)) {
                        return Some(cell);
                    }
                }
            }
        }
        None
    }

    /// Moves one creature's body a tick along its decision.
    fn move_creature(&mut self, id: u32, dt: f32) {
        let Some(creature) = self.creatures.live.get(&id) else { return };
        let species = self.content.creatures.get(creature.species);
        let mut body = creature.body;
        let decision = creature.decision;
        let held = self.tick < creature.held_until;
        let drive = match decision.goal {
            _ if held => Drive::default(),
            Some(goal) if goal.with_y(0.0).distance(body.position.with_y(0.0)) > 0.4 => {
                let wish = (goal - body.position).with_y(if species.def.habitat.water == Water::Swim { goal.y - body.position.y } else { 0.0 });
                Drive { wish: wish.normalize_or_zero(), speed: decision.speed, face: decision.face.map(|f| f - body.position) }
            }
            _ => Drive { wish: Vec3::ZERO, speed: 0.0, face: decision.face.map(|f| f - body.position) },
        };
        if held {
            // Held still: nothing carries it along, neither its legs nor a neighbour's push.
            body.velocity.x = 0.0;
            body.velocity.z = 0.0;
        }
        let before = body.position;
        body.reach = species.reach * creature.scale;
        body.head = species.head * creature.scale;
        motion::step(&mut body, &Ground(self), &species.def.movement, species.def.habitat.water, drive, dt);
        let creature = self.creatures.live.get_mut(&id).expect("still there");
        // Stuck against something for two seconds: give up the goal.
        if drive.wish != Vec3::ZERO && body.position.distance(before) < drive.speed * dt * 0.2 {
            creature.stuck += dt;
            if creature.stuck > 2.0 {
                creature.decision.goal = None;
                creature.mind.goal = None;
                creature.stuck = 0.0;
            }
        } else {
            creature.stuck = 0.0;
        }
        if body != creature.body {
            creature.body = body;
            creature.changed = true;
        }
        if body.position.y < -64.0 {
            self.creatures.live.remove(&id);
            self.creatures.gone.push(CreatureGone { id, died: false });
        }
    }

    /// Acts on what a creature decided.
    fn carry_out(&mut self, id: u32, decision: Decision) {
        let Some(creature) = self.creatures.live.get(&id) else { return };
        let species_id = creature.species;
        let position = creature.body.position;
        if let Some(player) = decision.attack {
            self.creature_strikes(id, player);
        }
        if let Some(cell) = decision.eat {
            // Grazing wears the land: now and then a tuft goes for good.
            let roll = unit_from(mix(self.creatures.live[&id].seed, self.tick ^ 0x6a2e));
            let block = self.block(cell);
            if roll < 0.3 && block.is_some_and(|b| self.content.blocks.info(b).replaceable && !self.content.blocks.info(b).solid) {
                self.set_block(cell, BlockId::AIR);
            }
        }
        if let Some(to) = decision.blink {
            let height = self.content.creatures.get(species_id).def.body.size;
            let landing = (0..6).map(|dy| (to + Vec3::Y * 3.0 - Vec3::Y * dy as f32).floor()).find(|cell| {
                let feet = cell.as_ivec3();
                self.is_solid(feet - IVec3::Y) && !Aabb::standing(cell.as_ivec3().as_vec3() + Vec3::new(0.5, 0.0, 0.5), height.0, height.1).intersects(|p| self.is_solid(p))
            });
            if let Some(cell) = landing {
                let creature = self.creatures.live.get_mut(&id).expect("listed");
                creature.body.position = cell + Vec3::new(0.5, 0.0, 0.5);
                creature.body.velocity = Vec3::ZERO;
                creature.changed = true;
            }
        }
        if let Some(at) = decision.leap {
            let creature = self.creatures.live.get_mut(&id).expect("listed");
            if creature.body.on_ground {
                let flat = (at - position).with_y(0.0);
                creature.body.velocity = flat.normalize_or_zero() * (flat.length() * 2.2).min(9.0) + Vec3::Y * 6.5;
                creature.changed = true;
            }
        }
        if decision.alarm {
            for (&other, kin) in self.creatures.live.iter_mut() {
                if other != id && kin.species == species_id && kin.body.position.distance(position) < KIN_RANGE {
                    kin.mind.alarmed();
                }
            }
        }
    }

    /// Creature `id` attacks a player: a hit rolled from its level against the player's
    /// Defence. A miss trains Defence only while the player fights this creature back: they
    /// landed a damaging blow on it lately, and it is a match for them (`DefenceTraining`,
    /// design wiki draft 63).
    fn creature_strikes(&mut self, id: u32, player: PlayerId) {
        let Some(creature) = self.creatures.live.get(&id) else { return };
        let (species, fought) = (creature.species, creature.fought);
        let def = &self.content.creatures.get(species).def;
        let Some(attack) = def.attack else { return };
        let Some(target) = self.players.get(&player) else { return };
        if !target.health.alive() || target.allow_flight {
            return;
        }
        let defence_skill = self.skill("core:defence");
        let defence = self.level(player, defence_skill);
        let rules = self.content.creatures.defence();
        let window = (rules.fought_within * TICKS_PER_SECOND as f32) as u64;
        let fighting = fought.is_some_and(|(by, at)| by == player && self.tick.saturating_sub(at) <= window);
        let trains = fighting && rules.matches(def.level, defence);
        let [hit, amount] = combat_roll(self.tick, species.0 as u32, player as u64 ^ 0xa7);
        let (low, high) = attack.damage;
        let target = self.players.get_mut(&player).expect("checked above");
        if hit < creature::hit_chance(def.level, defence) {
            let damage = low + ((amount * (high - low + 1) as f32) as u8).min(high - low);
            if target.health.damage(damage) {
                target.connection.send(ServerMessage::Died);
            }
            target.changed(traits::HEALTH);
        } else if let Some(skill) = defence_skill.filter(|_| trains) {
            // Experience for the blow turned aside: four a point of the damage it would do.
            let tenths = 40 * (low as u32 + high as u32) / 2;
            target.skills.grant(skill, tenths.max(10));
            target.changed(traits::SKILLS);
            target.connection.send(ServerMessage::Xp { skill, tenths: tenths.max(10) });
        }
    }

    /// A player strikes a creature (`core:attack`) with what they hold (`combat`): a hit
    /// rolled from the weapon's skill and accuracy against the creature's defence (half
    /// against the style it is weak to), damage up to the weapon's max hit for their level,
    /// experience in the weapon's skill and Hitpoints.
    pub(crate) fn player_strikes(&mut self, player: PlayerId, id: u32) {
        let Some(striker) = self.players.get(&player) else { return };
        let weapon = self.content.combat.wielded(striker.held(), |skill| self.content.skills.level(striker.skills.xp(skill)));
        // Timed by when the player's click arrived (see `actions`), so a busy server does
        // not shorten the wait between blows.
        let at = self.arrival;
        let since = self.creatures.struck.get(&player).map(|&last| at.saturating_duration_since(last).as_secs_f32());
        if since.is_some_and(|s| s < weapon.quickest() - 0.02) {
            return;
        }
        self.creatures.struck.insert(player, at);
        let strength = voxmar_engine::combat::strength_for(since.filter(|&s| s < 5.0), weapon.cooldown);
        let Some(creature) = self.creatures.live.get(&id) else { return };
        let species = self.content.creatures.get(creature.species);
        let attack_skill = weapon.skill;
        let attack = self.level(player, attack_skill);
        let defence = voxmar_engine::combat::defence_against(species.def.defence, species.def.weak, weapon.style);
        let [hit, amount] = combat_roll(self.tick, id, player as u64 ^ 0x5717);
        let creature = self.creatures.live.get_mut(&id).expect("checked above");
        if hit >= voxmar_engine::combat::hit_chance(attack, weapon.accuracy, defence, 0) {
            creature.mind.hurt(species, Some(player), creature.health as f32 / species.def.health as f32);
            if let Some(p) = self.players.get(&player) {
                p.connection.send(ServerMessage::Struck { creature: id, damage: 0, killed: false });
            }
            return;
        }
        // No knockback: the creature stays where the blow found it.
        creature.changed = true;
        let max_hit = weapon.max_hit(attack);
        let full = (1 + (amount * max_hit as f32) as u16).min(max_hit);
        // A blow struck too soon after the last lands lighter (`combat`).
        let damage = voxmar_engine::combat::scaled(full, strength, unit_from(mix(amount.to_bits() as u64, 0x5e1f))).min(creature.health);
        creature.health -= damage;
        creature.hurt_by = Some(player);
        // A blow that hurt it keeps the fight on, for Defence (`creature_strikes`).
        if damage > 0 {
            creature.fought = Some((player, self.tick));
        }
        creature.mind.hurt(species, Some(player), creature.health as f32 / species.def.health as f32);
        // RuneScape: four experience a point of damage in the style's skill, a third of
        // that in Hitpoints.
        let grants = [(attack_skill, 40 * damage as u32), (self.skill("core:hitpoints"), 13 * damage as u32)];
        if let Some(target) = self.players.get_mut(&player) {
            for (skill, tenths) in grants {
                if let Some(skill) = skill {
                    target.skills.grant(skill, tenths);
                    target.connection.send(ServerMessage::Xp { skill, tenths });
                }
            }
            target.changed(traits::SKILLS);
        }
        let killed = self.creatures.live[&id].health == 0;
        if let Some(p) = self.players.get(&player) {
            p.connection.send(ServerMessage::Struck { creature: id, damage, killed });
        }
        if killed {
            self.creature_dies(id);
        }
    }

    /// A creature dies: its loot pops out, its killer earns experience.
    pub(crate) fn creature_dies(&mut self, id: u32) {
        let Some(creature) = self.creatures.live.remove(&id) else { return };
        self.creatures.gone.push(CreatureGone { id, died: true });
        let species = self.content.creatures.get(creature.species);
        let killer = creature.hurt_by.and_then(|p| self.players.get(&p));
        let tool: Vec<String> = killer.and_then(|p| p.held()).map_or_else(Vec::new, |item| self.content.blocks.def(item).tags.clone());
        let bonus = match (killer, species.xp_skill) {
            (Some(p), Some(skill)) => level_bonus(self.content.skills.level(p.skills.xp(skill)), 1, self.content.skills.max_level()),
            _ => 0.0,
        };
        let moment = Moment { bonus, tool: &tool, night: self.daylight() < 0.3, by_player: killer.is_some() };
        let mut draws = 0u32;
        let found = loot::roll(&species.loot, &moment, || {
            draws += 1;
            unit_from(mix(creature.seed, draws as u64 ^ self.tick << 8))
        });
        let cell = creature.body.position.floor().as_ivec3();
        // The killer's journal: one more of the species, and each drop for the collection log.
        let mut journal = vec![(Self::page_of(species.def.kind), species.def.name.clone(), 1)];
        journal.extend(found.iter().map(|&(item, count)| (voxmar_engine::journal::Page::Collection, self.content.blocks.def(item).name.clone(), count as u32)));
        // Its loot comes out once its body has fallen, never from an animal still standing.
        let due = self.tick + (species.def.death.drops_after() * TICKS_PER_SECOND as f32).round() as u64;
        for (i, (item, count)) in found.into_iter().enumerate() {
            let body = dropped::popped(cell, random3(self.tick, cell, i as u32 + id));
            self.creatures.falling_loot.push((due, ItemStack { item, count }, body));
        }
        if let (Some(player), Some(skill)) = (creature.hurt_by, species.xp_skill) {
            let tenths = (species.def.xp.kill * 10.0) as u32;
            if let Some(p) = self.players.get_mut(&player).filter(|_| tenths > 0) {
                p.skills.grant(skill, tenths);
                p.changed(traits::SKILLS);
                p.connection.send(ServerMessage::Xp { skill, tenths });
            }
        }
        if let Some(player) = creature.hurt_by {
            for (page, name, times) in journal {
                self.discover(player, page, &name, times);
            }
        }
    }

    /// Drops the loot of creatures whose bodies have finished falling.
    fn drop_fallen_loot(&mut self) {
        let tick = self.tick;
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.creatures.falling_loot).into_iter().partition(|(due, ..)| *due <= tick);
        self.creatures.falling_loot = waiting;
        for (_, stack, body) in ready {
            self.items.spawn(stack, body, dropped::PICKUP_DELAY);
        }
    }

    /// Puts a creature of `species` at `at`, with a home there.
    pub(crate) fn spawn_creature(&mut self, species: CreatureId, at: Vec3, salt: u64) -> Option<u32> {
        if self.creatures.live.len() >= self.content.creatures.limits().most {
            return None;
        }
        let id = self.creatures.next;
        self.creatures.next += 1;
        let seed = mix(mix(self.meta.seed, id as u64), salt);
        let genome = self.wild_genome(species, at, seed);
        let def = &self.content.creatures.get(species).def;
        let traits = genes::express(&def.genes, &genome);
        // Size from its genes where it has a size gene; a little either way otherwise.
        let scale = if def.genes.iter().any(|g| matches!(g.kind, genes::GeneKind::Number { effect: genes::NumberEffect::Scale, .. })) {
            traits.scale
        } else {
            (0.88 + 0.24 * unit_from(mix(seed, 0x5ca1e))) * traits.scale
        };
        let size = (def.body.size.0 * scale, def.body.size.1 * scale);
        let yaw = unit_from(mix(seed, 3)) * std::f32::consts::TAU;
        // Its genes' temper moves its own personality toward fierce or calm.
        let mut mind = Mind::new(&def.temperament, seed);
        mind.personality.aggression = (mind.personality.aggression + traits.temper).clamp(0.0, 1.0);
        let creature = Live {
            species,
            body: Body::new(at, yaw, size),
            mind,
            health: def.health,
            home: at,
            decision: Decision::default(),
            scale,
            seed,
            hurt_by: None,
            fought: None,
            stuck: 0.0,
            owner: None,
            owner_id: None,
            trust: 0.0,
            trusting: None,
            ready_until: 0,
            held_until: 0,
            genome,
            traits,
            breeds_again: 0,
            grows_up: None,
            adult_scale: scale,
            products_due: def.products.iter().enumerate().map(|(i, p)| self.tick + product_ticks(p.every, seed, i)).collect(),
            fed: self.day,
            sent: None,
            changed: true,
        };
        self.creatures.live.insert(id, creature);
        Some(id)
    }

    /// A wild animal's genes, drawn from the population of the region `at` is in: each
    /// region has its own mix, pushed by its climate (docs/breeding.md, "Genes").
    pub(crate) fn wild_genome(&self, species: CreatureId, at: Vec3, seed: u64) -> genes::Genome {
        let defs = &self.content.creatures.get(species).def.genes;
        if defs.is_empty() {
            return genes::Genome::default();
        }
        let region = genes::region_of(at.x, at.z);
        let centre = (Vec2::new(region.0 as f32, region.1 as f32) + 0.5) * genes::REGION;
        let site = self.overworld.site(centre.x as i32, centre.y as i32);
        let name = &self.content.creatures.get(species).def.name;
        let key = name.bytes().fold(0u64, |h, b| mix(h, b as u64));
        genes::Population::of(defs, self.meta.seed, key, region, site.temperature, site.aether).individual(defs, seed)
    }

    /// Tries a spot around a player, and puts there a group of whatever suits it best,
    /// if anything does and the player has room for more around them.
    pub(crate) fn spawn_near(&mut self, player: PlayerId, salt: u64) {
        let Some(at) = self.players.get(&player).map(|p| p.position) else { return };
        let limits = self.content.creatures.limits();
        let wild_room = self.wild_room();
        if wild_room == 0 {
            return;
        }
        let nearby = |kind: Kind| {
            self.creatures
                .live
                .values()
                .filter(|c| c.body.position.distance(at) < NEAR && (self.content.creatures.get(c.species).def.kind == Kind::Monster) == (kind == Kind::Monster))
                .count()
        };
        let (animals, monsters) = (nearby(Kind::Animal), nearby(Kind::Monster));
        let seed = mix(mix(self.meta.seed, self.tick), salt ^ 0x5ba7);
        let angle = unit_from(seed) * std::f32::consts::TAU;
        let distance = SPAWN_NEAR + unit_from(mix(seed, 1)) * (SPAWN_FAR - SPAWN_NEAR);
        let x = (at.x + angle.cos() * distance).floor() as i32;
        let z = (at.z + angle.sin() * distance).floor() as i32;
        if !self.world.contains(ColumnPos::of_block(IVec3::new(x, 0, z))) {
            return;
        }
        let Some(spot) = self.standing_spot(x, z, at.y.floor() as i32) else { return };
        let site = self.overworld.site(x, z);
        let biome = self.overworld.biomes().get(site.biome);
        let underground = spot.y < site.height - 4;
        let dark = if underground { !self.lit_near(spot) } else { self.daylight() < 0.25 };
        let in_water = Ground(self).water_at(spot);
        let daylight = self.daylight();
        let mut choices: Vec<(CreatureId, f32)> = Vec::new();
        for (id, species) in self.content.creatures.iter() {
            let def = &species.def;
            let monster = def.kind == Kind::Monster;
            if (monster && (monsters >= limits.monsters_near || !dark)) || (!monster && animals >= limits.animals_near) {
                continue;
            }
            if def.habitat.caves != underground && !monster {
                continue;
            }
            let wet = matches!(def.habitat.water, Water::Swim);
            if wet != in_water {
                continue;
            }
            let weight = def.habitat.biome_weight(&biome.name, &biome.tags)
                * creature::fit(&def.habitat.comfort, site.temperature, site.moisture, site.aether)
                * spawn_factor(def.rarity)
                * def.habitat.active.liveliness(daylight);
            if weight > 0.0 {
                choices.push((id, weight));
            }
        }
        // Often nothing: most spots stay empty, so the land feels rare, not crowded.
        let total: f32 = choices.iter().map(|c| c.1).sum();
        // Animals are drawn half as often as they were, so the land fills slowly and gaps
        // stay; monsters as before.
        let nothing = if choices.iter().all(|&(id, _)| self.content.creatures.get(id).def.kind == Kind::Monster) { 2.0 } else { 5.0 };
        let mut pick = unit_from(mix(seed, 2)) * (total + nothing);
        let Some(&(species, _)) = choices.iter().find(|c| {
            pick -= c.1;
            pick <= 0.0
        }) else {
            return;
        };
        // Groups vary: even herd animals turn up alone or in pairs now and then.
        let group = self.content.creatures.get(species).def.group.size;
        let roll = unit_from(mix(seed, 4));
        let count = if roll < 0.18 {
            1
        } else if roll < 0.32 {
            group.1.min(2)
        } else {
            group.0 + (unit_from(mix(seed, 3)) * (group.1 - group.0 + 1) as f32) as u8
        };
        // Never past the cap: a herd arriving near it comes smaller.
        let room = if self.content.creatures.get(species).def.kind == Kind::Monster { limits.monsters_near.saturating_sub(monsters) } else { limits.animals_near.saturating_sub(animals) };
        let count = count.min(room.max(1).min(wild_room).min(u8::MAX as usize) as u8);
        for i in 0..count.max(1) {
            let offset = if i == 0 { IVec3::ZERO } else { IVec3::new((mix(seed, 10 + i as u64) % 7) as i32 - 3, 0, (mix(seed, 20 + i as u64) % 7) as i32 - 3) };
            let Some(cell) = self.standing_spot(spot.x + offset.x, spot.z + offset.z, spot.y) else { continue };
            self.spawn_creature(species, cell.as_vec3() + Vec3::new(0.5, 0.0, 0.5), seed ^ i as u64);
        }
    }

    /// Room left under the world's total: what `most` leaves after the living and the kept
    /// animals still waiting in the save, so those always fit back.
    pub(crate) fn room_left(&self) -> usize {
        self.content.creatures.limits().most.saturating_sub(self.creatures.live.len() + self.creatures.unplaced.len())
    }

    /// How many more wild creatures the world has room for: under the wild budget, which
    /// kept animals never touch, and under the total.
    pub(crate) fn wild_room(&self) -> usize {
        let wild = self.creatures.live.values().filter(|c| c.owner.is_none()).count();
        self.content.creatures.limits().wild_most.saturating_sub(wild).min(self.room_left())
    }

    /// How many animals a player keeps, alive or waiting in the save.
    pub(crate) fn kept_by(&self, name: &str) -> usize {
        let live = self.creatures.live.values().filter(|c| c.owner.as_deref() == Some(name)).count();
        live + self.creatures.unplaced.iter().filter(|s| s.owner == name).count()
    }

    /// How many young a keeper's animals may have now: room under the keeper's own cap and
    /// under the world's total.
    pub(crate) fn birth_room(&self, keeper: &str) -> usize {
        self.content.creatures.limits().kept_each.saturating_sub(self.kept_by(keeper)).min(self.room_left())
    }

    /// The cell nearest height `y` in a column where a body can stand: solid beneath, two
    /// free cells (or water) above.
    pub(crate) fn standing_spot(&self, x: i32, z: i32, y: i32) -> Option<IVec3> {
        let free = |cell: IVec3| self.block(cell).is_some_and(|b| !self.content.blocks.info(b).solid);
        let stands = |cell: IVec3| self.is_solid(cell - IVec3::Y) && free(cell) && free(cell + IVec3::Y);
        (0..24).flat_map(|d| [y + d, y - d - 1]).map(|cy| IVec3::new(x, cy, z)).find(|&cell| stands(cell))
    }

    /// Whether a block giving light lies within `LIT_REACH` of `cell`.
    fn lit_near(&self, cell: IVec3) -> bool {
        let r = LIT_REACH;
        (-r..=r).any(|dy| (-r..=r).any(|dz| (-r..=r).any(|dx| self.block(cell + IVec3::new(dx, dy, dz)).is_some_and(|b| self.content.blocks.info(b).emission > 0))))
    }

    /// Wild creatures far from every player, or in columns that went, go.
    fn despawn_far(&mut self) {
        let players: Vec<Vec3> = self.players.values().filter(|p| p.joined).map(|p| p.position).collect();
        let gone: Vec<u32> = self
            .creatures
            .live
            .iter()
            .filter(|(_, c)| c.owner.is_none())
            .filter(|(_, c)| {
                let far = players.iter().all(|p| p.distance(c.body.position) > DESPAWN);
                far || !self.world.contains(ColumnPos::of_block(c.body.position.floor().as_ivec3()))
            })
            .map(|(&id, _)| id)
            .collect();
        for id in gone {
            self.creatures.live.remove(&id);
            self.creatures.gone.push(CreatureGone { id, died: false });
        }
    }

    /// Sends each player the creatures that changed within sight, and those gone from it.
    fn replicate_creatures(&mut self) {
        let gone = std::mem::take(&mut self.creatures.gone);
        let every_other = self.tick.is_multiple_of(2);
        for player in self.players.values().filter(|p| p.joined) {
            let seen = self.creatures.seen.entry(player.id).or_default();
            let mut changed = Vec::new();
            let mut left: Vec<CreatureGone> = gone.iter().filter(|g| seen.remove(&g.id)).copied().collect();
            for (&id, creature) in &self.creatures.live {
                let near = creature.body.position.distance(player.position) < SEEN_WITHIN;
                let known = seen.contains(&id);
                let moved = creature.changed && creature.sent.is_none_or(|last| creature.moved_since(&last));
                if near && (!known || (every_other && moved)) {
                    seen.insert(id);
                    changed.push(creature.view(id));
                } else if !near && known {
                    seen.remove(&id);
                    left.push(CreatureGone { id, died: false });
                }
            }
            if !changed.is_empty() || !left.is_empty() {
                player.connection.send(ServerMessage::Creatures { changed, gone: left });
            }
        }
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        self.creatures.seen.retain(|id, _| ids.contains(id));
        if every_other {
            for creature in self.creatures.live.values_mut().filter(|c| c.changed) {
                if creature.sent.is_none_or(|last| creature.moved_since(&last)) {
                    creature.sent = Some(creature.view(0));
                }
                creature.changed = false;
            }
        }
    }
}

impl Ground<'_> {
    fn water_at(&self, cell: IVec3) -> bool {
        motion::Surroundings::water(self, cell)
    }
}

/// Ticks until a product is next due: its period, jittered a little by the creature and
/// product, so a flock does not lay all at once.
pub(crate) fn product_ticks(minutes: f32, seed: u64, index: usize) -> u64 {
    let jitter = 0.8 + 0.4 * unit_from(mix(seed, 0x9e0d ^ index as u64));
    (minutes * 60.0 * TICKS_PER_SECOND as f32 * jitter) as u64
}

/// How often a rarity spawns against a common species: the multipliers worldgen uses for
/// plants (`worldgen::rarity`).
fn spawn_factor(rarity: Rarity) -> f32 {
    match rarity {
        Rarity::Common => 1.0,
        Rarity::Uncommon => 0.3,
        Rarity::Rare => 0.08,
        Rarity::VeryRare => 0.015,
        Rarity::Legendary => 0.002,
    }
}

/// The `i`th cell of the square ring `r` blocks out, as (dx, dz).
fn ring(r: i32, i: i32) -> (i32, i32) {
    if r == 0 {
        return (0, 0);
    }
    let side = 2 * r;
    let (edge, t) = (i / side, i % side - r);
    match edge {
        0 => (t, -r),
        1 => (r, t),
        2 => (-t, r),
        _ => (-r, -t),
    }
}


/// A blow's two rolls (whether it lands, and how hard), from the tick and who strikes whom.
/// A strong mix of all three: blows come at a steady beat, so a weak one (the tick folded in
/// once, against a creature standing in one cell) gave long runs of misses or of hits, a
/// level-1 snail taking anywhere from 4 to 40 swings.
pub(crate) fn combat_roll(tick: u64, creature: u32, striker: u64) -> [f32; 2] {
    let h = mix(mix(mix(tick, creature as u64), striker), 0xc0b);
    [unit_from(h), unit_from(mix(h, 1))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rings_cover_their_squares() {
        let mut seen = FxHashSet::default();
        for r in 0..=3 {
            for i in 0..(8 * r).max(1) {
                let (dx, dz) = ring(r, i);
                assert_eq!(dx.abs().max(dz.abs()), r);
                assert!(seen.insert((dx, dz)));
            }
        }
        assert_eq!(seen.len(), 49);
    }
}

#[cfg(test)]
mod combat_tests {
    use super::*;

    #[test]
    fn defence_trains_only_from_a_creature_the_player_lately_hurt_and_a_match() {
        let mut server = crate::test_server();
        let (_client, connection) = voxmar_protocol::local_pair();
        let (player, other) = (1, 2);
        let mut joined = crate::player::Player::new(player, connection, server.spawn, 2);
        joined.joined = true;
        server.players.insert(player, joined);
        server.tick = 10_000;
        // Defence 20: a wolf (level 12) is a match for it, a fox (level 4) is not.
        let defence = server.content.skills.id("core:defence").expect("Defence");
        let xp_for = server.content.skills.xp_for(20);
        server.players.get_mut(&player).unwrap().skills.grant(defence, xp_for);
        let (wolf, fox) = (server.content.creatures.id("base:wolf").unwrap(), server.content.creatures.id("base:fox").unwrap());
        let at = server.spawn;
        let [a, b] = [1, 2].map(|salt| server.spawn_creature(wolf, at + Vec3::X, salt).expect("room"));
        let small = server.spawn_creature(fox, at + Vec3::X, 3).expect("room");
        // A hundred blows from `attacker` at the player, whose fight with it is `fought`
        // (who landed a blow, and how many ticks before each): the Defence xp they earn.
        let earned = |server: &mut Server, attacker: u32, fought: Option<(PlayerId, u64)>| {
            let before = server.players[&player].skills.xp(defence);
            for _ in 0..100 {
                server.tick += 7;
                let tick = server.tick;
                server.creatures.live.get_mut(&attacker).unwrap().fought = fought.map(|(by, ago)| (by, tick - ago));
                server.players.get_mut(&player).unwrap().health = traits::Health::full(crate::player::MAX_HEALTH);
                server.creature_strikes(attacker, player);
            }
            server.players[&player].skills.xp(defence) - before
        };
        let within = (server.content.creatures.defence().fought_within * TICKS_PER_SECOND as f32) as u64;
        assert!(earned(&mut server, a, Some((player, within))) > 0, "fighting a wolf back trained nothing");
        assert_eq!(earned(&mut server, a, None), 0, "standing among wolves trained Defence");
        assert_eq!(earned(&mut server, a, Some((player, within + 1))), 0, "a fight over for more than 10 s still trained");
        assert_eq!(earned(&mut server, a, Some((other, 0))), 0, "another player's fight trained this one");
        assert_eq!(earned(&mut server, small, Some((player, 0))), 0, "a fox, under half the player's Defence, trained it");
        // Hurting one wolf trains nothing against another.
        server.creatures.live.get_mut(&b).unwrap().fought = Some((player, server.tick + 10_000));
        assert_eq!(earned(&mut server, a, None), 0, "a blow at another wolf trained Defence");
        // Real blows: one that misses starts no fight, one that lands does.
        server.creatures.live.get_mut(&a).unwrap().fought = None;
        let (mut missed, mut landed) = (false, false);
        for _ in 0..200 {
            server.tick += 1;
            server.arrival += std::time::Duration::from_secs(2);
            let health = server.creatures.live[&a].health;
            server.player_strikes(player, a);
            let wolf_now = &server.creatures.live[&a];
            if wolf_now.health < health {
                assert_eq!(wolf_now.fought, Some((player, server.tick)), "a landed blow started no fight");
                landed = true;
                server.creatures.live.get_mut(&a).unwrap().fought = None;
            } else {
                assert_eq!(wolf_now.fought, None, "a blow that missed started a fight");
                missed = true;
            }
            if missed && landed {
                break;
            }
        }
        assert!(missed && landed, "200 blows at a wolf all missed or all landed");
    }

    #[test]
    fn blows_at_a_steady_beat_land_as_often_as_the_odds_say() {
        // A player swinging every seven ticks at a level-1 creature that stands still.
        let chance = creature::hit_chance(1, 1);
        for creature in 0..20u32 {
            let (mut landed, mut misses, mut longest) = (0, 0, 0);
            for blow in 0..400u64 {
                let [hit, _] = combat_roll(1000 + blow * 7, creature, 1 ^ 0x5717);
                if hit < chance {
                    landed += 1;
                    misses = 0;
                } else {
                    misses += 1;
                    longest = longest.max(misses);
                }
            }
            let rate = landed as f32 / 400.0;
            assert!((rate - chance).abs() < 0.08, "creature {creature}: {rate} landed, the odds are {chance}");
            assert!(longest < 20, "creature {creature}: {longest} misses in a row");
        }
    }
}
