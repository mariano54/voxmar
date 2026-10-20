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
use voxmar_engine::creature::condition::Needs;
use voxmar_engine::creature::motion::{self, Body, Drive};
use voxmar_engine::creature::path;
use voxmar_engine::creature::{self, CreatureId, Kind, Water, mix, unit_from};
use voxmar_engine::crop::level_bonus;
use voxmar_engine::dropped;
use voxmar_engine::genes;
use voxmar_engine::glam::{IVec3, Quat, Vec2, Vec3};
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::loot::{self, Moment};
use voxmar_engine::physics::Aabb;
use voxmar_engine::skill::SkillId;
use voxmar_engine::traits;
use voxmar_protocol::{CreatureGone, CreatureView, PlayerId, ServerMessage, TICKS_PER_DAY, TICKS_PER_SECOND};

use crate::save::Draw;
use crate::Server;

/// Creatures think every this many ticks within `NEAR` blocks of a player, every
/// `FAR_THINK` beyond.
const NEAR_THINK: u64 = 4;
/// How many beats creatures' thoughts are spread over: each thinks `seed % THINK_SPREAD`
/// ticks less often than `NEAR_THINK` or `FAR_THINK`, so neighbours do not all decide at once.
const THINK_SPREAD: u64 = 3;
/// The longest a creature near a player goes between thoughts, in ticks: `NEAR_THINK` and
/// the most its seed adds. It acts on what it sees only at a thought: it starts a chase, and
/// strikes once in reach, each up to this late.
pub const NEAR_THINK_LONGEST: u64 = NEAR_THINK + THINK_SPREAD - 1;
/// What tells a creature's own rolls apart (`Live::rolls`): a thought's, a product's timing.
pub(crate) const THOUGHT: u64 = 0x7409;
pub(crate) const PRODUCT: u64 = 0x9d0c;

/// The roll a creature of `seed` draws at its own count `rolls` for a kind (`THOUGHT`,
/// `PRODUCT`): the kind's salt nested round the count, so no count's draw of one kind is
/// another count's draw of the other.
pub(crate) fn own_roll(seed: u64, rolls: u64, kind: u64) -> u64 {
    mix(mix(seed, rolls), kind)
}
pub(crate) const FAR_THINK: u64 = 20;
const NEAR: f32 = 64.0;
/// Wild creatures further than this from every player go.
const DESPAWN: f32 = 128.0;
/// Players are told about creatures within this many blocks.
const SEEN_WITHIN: f32 = 96.0;
/// How far kin hear an alarm, and are counted as kin by herding.
const KIN_RANGE: f32 = 14.0;
/// The width of a `Neighbours` cell, in blocks: the widest search (kin) and a block more for
/// how far a creature moves in a pass, so the three by three cells around one hold every
/// creature in range. This holds while no creature moves more than a block in a pass: a
/// species that blinks (5 to 8 blocks at once) would need a wider margin. None does today.
const CELL: f32 = 16.0;
const _: () = assert!(KIN_RANGE + 1.0 <= CELL && voxmar_engine::creature::husbandry::MATE_RANGE + 1.0 <= CELL);
/// How far creatures look for food.
const FOOD_RANGE: i32 = 6;
/// Seconds a player must keep within `SETTLE_RADIUS` blocks of one spot before hunters
/// there leave them be (RuneScape's ten minutes in 21 × 21 squares).
pub(crate) const TOLERANCE: u64 = 600 * TICKS_PER_SECOND as u64;
const SETTLE_RADIUS: f32 = 10.0;
/// Light from blocks within this many blocks keeps monsters away underground.
const LIT_REACH: i32 = 5;
/// Where a creature's stuck clock (`Live::stuck`, to 2 s) stands once a search finds no way
/// to its goal.
const NO_WAY_STUCK: f32 = 1.5;
/// How soon after balking at a chase a second balk makes a creature give it up, in ticks.
const BALK_WINDOW: u64 = 8 * TICKS_PER_SECOND as u64;
/// How long after giving a chase up a creature that takes one up again gives it up at its
/// first balk, in ticks: it has been round that box already.
const REBALK_WINDOW: u64 = 60 * TICKS_PER_SECOND as u64;
/// How far a land animal out of its depth looks for the shore.
const SHORE_RANGE: i32 = 12;
/// How near a wild creature comes to a player holding its favourite food to count as drawn
/// to them (`Server::note_lured`): about a blow's reach.
const LURED_WITHIN: f32 = 3.0;

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
    /// Rolls of its own drawn so far (each thought's, each product's timing), so its next
    /// is numbered by them, never by the tick it falls on. Saved with a kept one.
    pub rolls: u64,
    /// The last player to hurt it, for the kill's loot and experience.
    pub hurt_by: Option<PlayerId>,
    /// The last player to land a damaging blow on it and the tick they did: its blows that
    /// miss train that player's Defence for a while (`DefenceTraining`). With two players
    /// at it, only the last hitter's is kept. Not saved.
    pub fought: Option<(PlayerId, u64)>,
    /// Once kept and let go (`release`): taming it again pays no Hunter xp and counts for
    /// no quest. Only wild creatures carry it, and those are never saved.
    pub was_kept: bool,
    /// Drawn up close by a player holding its favourite food (`note_lured`): it counts for
    /// their quests once. Not saved.
    pub lured: bool,
    /// Came with a wandering party (`wandering`), and so is never one of the settlements'
    /// people, in the party or let go from it.
    pub wanderer: bool,
    /// A wanderer led to a player's home and left to live there (`wandering`): saved with
    /// the world and never despawned for distance.
    pub companion: bool,
    /// Seconds without getting nearer its goal, to notice being stuck.
    pub stuck: f32,
    /// The side it is going round something in its way to (`motion::steer`): 1 or -1, 0
    /// while its way is open.
    pub detour: f32,
    /// The short way round it is following (`path::find`), cell by cell; the goal cell it
    /// was found for, and when it last looked.
    pub way: Vec<IVec3>,
    pub way_to: Option<IVec3>,
    pub way_at: u64,
    /// Out of its depth (a land animal), the dry cell it makes for, and the tick it last
    /// looked for one. Not saved.
    pub shore: Option<IVec3>,
    pub shore_at: Option<u64>,
    /// Times running a chase of its balked (gave up its goal, stuck), and the tick of the
    /// last. Not saved.
    pub balked: u8,
    pub balked_at: u64,
    /// When it last gave a chase up for balking (`BALK_WINDOW`). Not saved.
    pub gave_up_at: Option<u64>,
    /// The player who keeps it, by name (tamed or bred), and the id they have now.
    pub owner: Option<String>,
    pub owner_id: Option<PlayerId>,
    /// Trust in the player feeding it, toward taming (1 is tame), and who that is.
    pub trust: f32,
    pub trusting: Option<String>,
    /// Tick it stops being ready to breed, and the tick it may breed again.
    pub ready_until: u64,
    /// The tick until which it stands still, for an admin's `/creatures hold`, a wind-up,
    /// recovery or a stagger (`fight`).
    pub held_until: u64,
    /// Winding up an attack (`fight`): shown in the attacking pose until it lands.
    pub winding: bool,
    /// Following the player it winds up at, until a tick, before its aim locks (`fight`).
    pub tracking: Option<(PlayerId, u64)>,
    /// Running a charge it wound up (`fight`, `creature::moves::Charge`): straight on, held
    /// from thinking until it ends.
    pub charging: Option<crate::fight::Charging>,
    /// The attack it winds up or last used (`Creature::attack_at`): 0 its plain one, then
    /// its moves (`fight`).
    pub attack: u8,
    /// The phase of its fight it has come to (`Creature::phase`), and whether it rears on
    /// coming to one (held still until `held_until`).
    pub phase: u8,
    pub rearing: bool,
    /// Staggered (`fight`): held still until `held_until`, shown reeling (`STAGGERED`).
    pub staggered: bool,
    /// The tick it last mended a wound (`lairs`).
    pub mended: u64,
    /// Its genes, and what shows of them.
    pub genome: genes::Genome,
    pub traits: genes::Traits,
    pub breeds_again: u64,
    /// Tick it grows up, while young; its grown size.
    pub grows_up: Option<u64>,
    pub adult_scale: f32,
    /// Tick each of its species' products is next due.
    pub products_due: Vec<u64>,
    /// How well it is kept, 0 to 1 (crate::condition): the day clock when it was last
    /// reckoned and when it last ate, and its needs as last looked at.
    pub condition: f32,
    pub reckoned: f64,
    pub fed: f64,
    pub needs: Needs,
    /// How well its home suits its kind's climate (`Server::settle`).
    pub climate_fit: f32,
    /// The last view each player was sent, to send only changes.
    sent: Option<CreatureView>,
    pub(crate) changed: bool,
}

impl Live {
    /// Its pose as players see it: attacking all through a wind-up.
    fn pose(&self) -> brain::Pose {
        if self.winding || self.charging.is_some() { brain::Pose::Attacking } else { self.decision.pose }
    }

    /// The tell it shows (`CreatureView::tell`): its move while it winds one up, `REARING`
    /// while it rears into a new phase, `STAGGERED` while it reels from a stagger, else none.
    pub(crate) fn tell(&self) -> u8 {
        if self.rearing {
            voxmar_protocol::REARING
        } else if self.staggered {
            voxmar_protocol::STAGGERED
        } else if self.winding {
            self.attack
        } else {
            0
        }
    }

    pub(crate) fn view(&self, id: u32) -> CreatureView {
        CreatureView {
            id,
            species: self.species,
            position: self.body.position,
            velocity: self.body.velocity,
            yaw: self.body.yaw,
            pose: self.pose(),
            tell: self.tell(),
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
            || last.pose != self.pose()
            || last.tell != self.tell()
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
    pub(crate) gone: Vec<CreatureGone>,
    /// Where each player settled and since when (RuneScape's tolerance).
    pub(crate) settled: FxHashMap<PlayerId, (Vec3, u64)>,
    /// Kept creatures from the save that could not be put back yet (their species is gone
    /// from the content, or the world was full): kept in the save and tried again, never
    /// lost.
    pub unplaced: Vec<crate::husbandry::SavedCreature>,
    /// Loot waiting for its body to finish falling: the tick it drops, and what.
    pub(crate) falling_loot: Vec<(u64, ItemStack, dropped::Loose)>,
    /// The animal each player last asked to let go, and the tick: a second ask soon after
    /// lets it go (`release`).
    pub(crate) release_asked: FxHashMap<PlayerId, (u32, u64)>,
    /// Which blocks are water a creature swims in or drinks, by id, worked out once.
    water: std::sync::OnceLock<Vec<bool>>,
    /// Which blocks are railings (`RAILING_TAG`), by id, worked out once.
    railings: std::sync::OnceLock<Vec<bool>>,
}

/// Every live creature by where it stood when a pass began, in square cells `CELL` blocks
/// across, so finding those near one looks at nine cells instead of every creature in the
/// world: kin for herding and alarms, a mate. Each id stands with its species, so a search
/// for one kind passes the others by without looking them up: this holds as a creature's
/// id and species never change once it is spawned.
pub(crate) struct Neighbours {
    cells: FxHashMap<(i32, i32), Vec<(u32, CreatureId)>>,
    /// Those of a kind in the nine cells around a cell, in id order, for each cell and kind
    /// asked about so far: in a full pen every one of a kind asks for the same cell at each
    /// think, so a pass gathers and sorts them once.
    kinds: std::cell::RefCell<FxHashMap<((i32, i32), CreatureId), std::rc::Rc<[u32]>>>,
}

impl Neighbours {
    pub(crate) fn of(live: &BTreeMap<u32, Live>) -> Self {
        let mut cells: FxHashMap<(i32, i32), Vec<(u32, CreatureId)>> = FxHashMap::default();
        for (&id, creature) in live {
            cells.entry(Self::cell(creature.body.position)).or_default().push((id, creature.species));
        }
        Self { cells, kinds: Default::default() }
    }

    fn cell(at: Vec3) -> (i32, i32) {
        ((at.x / CELL).floor() as i32, (at.z / CELL).floor() as i32)
    }

    /// The creatures of `species` but `id` that stood in the cells around `at`, in id order
    /// (as the world's own order, so what is found does not change with the search): every
    /// one within `CELL` blocks less a pass's movement, and some further.
    pub(crate) fn kind_around(&self, at: Vec3, species: CreatureId, id: u32) -> impl Iterator<Item = u32> {
        let (cx, cz) = Self::cell(at);
        let known = self.kinds.borrow().get(&((cx, cz), species)).cloned();
        let ids = known.unwrap_or_else(|| {
            let mut ids: Vec<u32> = (-1..=1)
                .flat_map(|dx| (-1..=1).map(move |dz| (cx + dx, cz + dz)))
                .filter_map(|cell| self.cells.get(&cell))
                .flatten()
                .filter(|&&(_, kind)| kind == species)
                .map(|&(other, _)| other)
                .collect();
            ids.sort_unstable();
            let ids: std::rc::Rc<[u32]> = ids.into();
            self.kinds.borrow_mut().insert(((cx, cz), species), ids.clone());
            ids
        });
        (0..ids.len()).map(move |i| ids[i]).filter(move |&other| other != id)
    }
}

impl Creatures {
    /// Whether `player` was last told of creature `id`: it is in their sight.
    pub(crate) fn sees(&self, player: PlayerId, id: u32) -> bool {
        self.seen.get(&player).is_some_and(|seen| seen.contains(&id))
    }

    /// Which blocks are water (a fluid named for it), by id.
    fn water_blocks(&self, blocks: &voxmar_engine::block::BlockRegistry) -> &[bool] {
        self.water.get_or_init(|| blocks.ids().map(|b| blocks.info(b).fluid.is_some() && blocks.def(b).name.contains("water")).collect())
    }

    /// Which blocks are railings, by id: to a creature a railing stands two high, as a
    /// fence does, so nothing hops onto it and over the edge it guards.
    fn railing_blocks(&self, blocks: &voxmar_engine::block::BlockRegistry) -> &[bool] {
        self.railings.get_or_init(|| blocks.ids().map(|b| blocks.def(b).has_tag(voxmar_engine::block::RAILING_TAG)).collect())
    }

    /// Takes a creature out of the world without a death (`/creatures remove`).
    pub(crate) fn take_away(&mut self, id: u32) {
        if self.live.remove(&id).is_some() {
            self.gone.push(CreatureGone { id, died: false });
        }
    }

    /// The box a blow at a creature may land in: its own, widened by how far it moves in
    /// `lag` seconds, which a player's view of it may run behind (`Feel::reach_lag`; the
    /// reach itself adds `Feel::reach_slack`).
    pub fn reach_box(&self, id: u32, lag: f32) -> Option<Aabb> {
        let creature = self.live.get(&id).filter(|c| c.health > 0)?;
        let mut aabb = creature.body.aabb();
        let slack = creature.body.velocity.length() * lag.max(0.0);
        aabb.min -= Vec3::splat(slack);
        aabb.max += Vec3::splat(slack);
        Some(aabb)
    }
}

/// The world as a creature's body feels it. A body, or a search for a way, looks at cells
/// side by side, so it keeps the column it last looked in rather than find it again.
struct Ground<'a> {
    server: &'a Server,
    last: std::cell::Cell<Option<(ColumnPos, &'a voxmar_engine::world::Column)>>,
    /// Whether a railing is the block-high step it is (`Ground::climbing`), not a wall two
    /// high.
    climbs_railings: bool,
}

impl<'a> Ground<'a> {
    fn of(server: &'a Server) -> Self {
        Self { server, last: std::cell::Cell::new(None), climbs_railings: false }
    }

    /// The world as a creature that climbs a railing feels it: a boss, or a fighter after a
    /// player, gets over a railing as a player does, so a ring of railings is no pen it
    /// presses on while a spear outreaches it. Grazers, wanderers and kept animals keep to
    /// `Ground::of`, where a railing stands two high.
    fn climbing(server: &'a Server, climbs: bool) -> Self {
        Self { climbs_railings: climbs, ..Self::of(server) }
    }

    /// Whether the block in `cell` is a railing.
    fn railing(&self, cell: IVec3) -> bool {
        self.block(cell).is_some_and(|b| self.server.creatures.railing_blocks(&self.server.content.blocks)[b.0 as usize])
    }

    /// Whether the block in `cell` is solid (an unloaded column is).
    fn solid_block(&self, cell: IVec3) -> bool {
        let blocks = &self.server.content.blocks;
        self.block(cell).is_none_or(|b| blocks.info(b).solid)
    }

    /// As `Server::block`: none in an unloaded column or under the world.
    fn block(&self, cell: IVec3) -> Option<BlockId> {
        if cell.y >= voxmar_engine::coords::WORLD_HEIGHT {
            return Some(BlockId::AIR);
        }
        if cell.y < 0 {
            return None;
        }
        let at = ColumnPos::of_block(cell);
        let column = match self.last.get() {
            Some((pos, column)) if pos == at => column,
            _ => {
                let column: &'a voxmar_engine::world::Column = self.server.world.column(at)?;
                self.last.set(Some((at, column)));
                column
            }
        };
        let size = voxmar_engine::coords::CHUNK_SIZE;
        Some(column.block(IVec3::new(cell.x & (size - 1), cell.y, cell.z & (size - 1))))
    }
}

impl motion::Surroundings for Ground<'_> {
    fn solid(&self, cell: IVec3) -> bool {
        // Unloaded columns are walls, so nothing walks off the edge of the world. Over a
        // railing is a wall too, to all but a climber: it stands two high to a creature.
        self.solid_block(cell) || !self.climbs_railings && self.railing(cell - IVec3::Y)
    }

    fn footing(&self, cell: IVec3) -> bool {
        // The wall over a railing is no floor, and nor, to all but a climber, is a
        // railing's top: nothing walks on air along it or hops over it from beside it.
        self.solid_block(cell) && (self.climbs_railings || !self.railing(cell))
    }

    fn water(&self, cell: IVec3) -> bool {
        self.block(cell).is_some_and(|b| self.server.creatures.water_blocks(&self.server.content.blocks)[b.0 as usize])
    }
}

impl Server {
    fn skill(&self, name: &str) -> Option<SkillId> {
        self.content.skills.id(name)
    }

    pub(crate) fn level(&self, player: PlayerId, skill: Option<SkillId>) -> u8 {
        let Some(skill) = skill else { return 1 };
        self.players.get(&player).map_or(1, |p| self.content.skills.level(p.skills.xp(skill)))
    }

    /// A player's combat level, RuneScape-style (see `SkillRegistry::combat_level`).
    pub(crate) fn combat_level(&self, player: PlayerId) -> u8 {
        let skills = &self.content.skills;
        self.players.get(&player).map_or_else(|| skills.combat_level(&Default::default()), |p| skills.combat_level(&p.skills))
    }

    /// Daylight now, 0..=1.
    pub(crate) fn daylight(&self) -> f32 {
        creature::daylight(self.day as u64)
    }

    /// Whether the sun is down now (`Condition::Night`).
    pub(crate) fn night(&self) -> bool {
        let ticks = voxmar_protocol::TICKS_PER_DAY;
        voxmar_engine::calendar::night((self.day as u64 % ticks) as f64 / ticks as f64)
    }

    /// Ticks between spawn attempts around each player (`limits.spawn_every` seconds).
    fn spawn_every_ticks(&self) -> u64 {
        ((self.content.creatures.limits().spawn_every * TICKS_PER_SECOND as f32).round() as u64).max(1)
    }

    /// Moves, thinks for, spawns and despawns creatures, then tells players.
    pub(crate) fn tick_creatures(&mut self) {
        let _span = tracing::info_span!("creatures").entered();
        self.settle_players();
        if self.tick.is_multiple_of(self.spawn_every_ticks()) {
            let ids: Vec<PlayerId> = self.players.values().filter(|p| p.joined).map(|p| p.id).collect();
            for (i, id) in ids.into_iter().enumerate() {
                self.spawn_near(id, i as u64);
            }
            self.despawn_far();
        }
        self.tick_villagers();
        self.tick_wandering();
        self.think_and_move();
        self.note_lured();
        self.tick_husbandry();
        self.drop_fallen_loot();
        if self.tick.is_multiple_of(10 * TICKS_PER_SECOND as u64) {
            self.place_unplaced();
        }
        self.replicate_creatures();
    }

    /// Notes where each player has stayed, for tolerance, and forgets the asks of those who left.
    fn settle_players(&mut self) {
        let tick = self.tick;
        let players: Vec<(PlayerId, Vec3)> = self.players.values().filter(|p| p.joined).map(|p| (p.id, p.position)).collect();
        let settled = &mut self.creatures.settled;
        settled.retain(|id, _| players.iter().any(|(p, _)| p == id));
        self.creatures.release_asked.retain(|id, _| players.iter().any(|(p, _)| p == id));
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

    /// Once a second: a wild creature within `LURED_WITHIN` blocks of a player holding its
    /// favourite food was drawn there by it, and the player's `lured` tally counts it, once
    /// for each creature.
    fn note_lured(&mut self) {
        if !self.tick.is_multiple_of(TICKS_PER_SECOND as u64) {
            return;
        }
        let mut lured: Vec<(u32, PlayerId, String)> = Vec::new();
        for (&id, creature) in &self.creatures.live {
            if creature.lured || creature.owner.is_some() {
                continue;
            }
            let species = self.content.creatures.get(creature.species);
            let at = creature.body.position;
            let drawn = |p: &&crate::player::Player| p.joined && p.health.alive() && p.position.distance(at) < LURED_WITHIN && p.held().is_some_and(|item| species.favourite.contains(&item));
            if let Some(player) = self.players.values().find(drawn) {
                lured.push((id, player.id, species.def.name.clone()));
            }
        }
        for (id, player, name) in lured {
            if let Some(creature) = self.creatures.live.get_mut(&id) {
                creature.lured = true;
            }
            self.tally(player, "lured", &name, 1);
        }
    }

    pub(crate) fn think_and_move(&mut self) {
        let dt = 1.0 / TICKS_PER_SECOND as f32;
        let ids: Vec<u32> = self.creatures.live.keys().copied().collect();
        let neighbours = Neighbours::of(&self.creatures.live);
        let daylight = self.daylight();
        let handoff = self.overworld.settlement_people().live.handoff;
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
            let every = if nearest < NEAR { NEAR_THINK } else { FAR_THINK } + creature.seed % THINK_SPREAD;
            if (self.tick + id as u64).is_multiple_of(every) {
                let range = species.def.senses.sight.max(species.def.senses.hearing).max(16.0) * 1.5;
                let players = self.seen_from(position, range, &species.favourite, creature.owner.as_deref());
                let mate = self.mate_for(id, &neighbours);
                let kin = self.kin_of(id, &neighbours);
                // What it grazes, else, kept and hungry, a trough of its food by home.
                let food = if species.eats.is_empty() { None } else { self.food_near(position, &species.eats, creature.seed) }.or_else(|| self.trough_for(id));
                let situation = Situation {
                    position,
                    home: creature.home,
                    health: creature.health as f32 / species.def.health as f32,
                    phase: creature.phase,
                    scale: creature.scale,
                    daylight,
                    players: &players,
                    kin: &kin,
                    food,
                    mate,
                    kept: creature.owner.is_some(),
                    errand: self.villagers.errand(id, handoff).or_else(|| self.wandering.errand(id)),
                    dt: every as f32 * dt,
                    // By its own count of thoughts, never the tick it thinks on.
                    seed: own_roll(creature.seed, creature.rolls, THOUGHT),
                };
                let wet = Ground::of(self).water_at(position.floor().as_ivec3());
                let creature = self.creatures.live.get_mut(&id).expect("listed");
                creature.rolls += 1;
                // A charge's wind-up holds him to its locked line until he sets off, through
                // the wait for the player's round trip past the wind-up (`lag_allowance`).
                let winding_a_charge = creature.winding && species.attack_at(creature.attack).is_some_and(|a| a.charge.is_some());
                let decision = if self.tick < creature.held_until || creature.charging.is_some() || winding_a_charge {
                    brain::Decision { pose: brain::Pose::Idle, ..Default::default() }
                } else {
                    // Done rearing into a new phase, or reeling from a stagger: it fights on
                    // (the view goes out below, its tell changed).
                    creature.rearing = false;
                    creature.staggered = false;
                    brain::think(species, &mut creature.mind, &situation)
                };
                // Led by food held up (how a wild one is lured home, or a kept one moved to a
                // new pen), its home goes with it: once the player puts the food away it
                // stays where it was led rather than walk back. A land animal led through water
                // keeps the last dry place as its home.
                let land = species.def.habitat.water == Water::Land;
                let wading = land && (creature.body.in_water || wet || !creature.body.on_ground);
                if situation.errand.is_none() && !wading && creature.mind.doing.is_some_and(|i| species.modules[i].0.name() == "tempted") {
                    creature.home = creature.body.position;
                }
                let wandering = creature.mind.doing.is_some_and(|i| species.modules[i].0.name() == "wander");
                creature.decision = decision;
                creature.changed = true;
                // A land animal does not wander off into a lake: a stroll that would end over
                // water is dropped, and the next look picks another.
                let mut decision = decision;
                let ground = Ground::of(self);
                if land && wandering && decision.goal.is_some_and(|g| ground.water_at(g.floor().as_ivec3()) || ground.water_at(g.floor().as_ivec3() - IVec3::Y)) {
                    decision.goal = None;
                    let creature = self.creatures.live.get_mut(&id).expect("listed");
                    (creature.mind.goal, creature.decision) = (None, decision);
                }
                actions.push((id, decision));
            }
            self.move_creature(id, dt);
        }
        self.push_apart();
        for (id, decision) in actions {
            self.carry_out(id, decision, &neighbours);
        }
    }

    /// Where creature `id`'s kin are: others of its kind within `KIN_RANGE`, in id order.
    pub(crate) fn kin_of(&self, id: u32, neighbours: &Neighbours) -> Vec<Vec3> {
        let Some(creature) = self.creatures.live.get(&id) else { return Vec::new() };
        let (species, at) = (creature.species, creature.body.position);
        neighbours
            .kind_around(at, species, id)
            .filter_map(|other| self.creatures.live.get(&other))
            .filter(|c| c.species == species && c.body.position.distance(at) < KIN_RANGE)
            .map(|c| c.body.position)
            .collect()
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
    fn food_near(&self, at: Vec3, eats: &[BlockId], seed: u64) -> Option<IVec3> {
        let feet = at.floor().as_ivec3();
        // A few rings out, starting at a side of its own (by its seed) so neighbours don't
        // all pick the same tuft.
        let start = (mix(seed, 0xf00d) % 4) as i32;
        let ground = Ground::of(self);
        for r in 0..=FOOD_RANGE {
            for i in 0..(8 * r).max(1) {
                let (dx, dz) = ring(r, (i + start * r.max(1)) % (8 * r).max(1));
                for dy in [0, -1, 1] {
                    let cell = feet + IVec3::new(dx, dy, dz);
                    if ground.block(cell).is_some_and(|b| eats.contains(&b)) {
                        return Some(cell);
                    }
                }
            }
        }
        None
    }

    /// The nearest dry cell, within `SHORE_RANGE` blocks of `at` and a block of its level,
    /// where a body `height` blocks tall stands.
    fn shore_for(&self, at: Vec3, height: i32) -> Option<IVec3> {
        let ground = Ground::of(self);
        let walker = path::Walker { height, climbs: true, wary: true, span: 1 };
        let feet = at.floor().as_ivec3();
        (1..=SHORE_RANGE).find_map(|r| {
            (-r..=r)
                .flat_map(|x| (-r..=r).map(move |z| (x, z)))
                .filter(|&(x, z)| x.abs() == r || z.abs() == r)
                .flat_map(|(x, z)| (-1..=1).map(move |y| feet + IVec3::new(x, y, z)))
                .filter(|&cell| !ground.water_at(cell) && walker.stands(&ground, cell))
                .min_by(|a, b| (a.as_vec3() + Vec3::splat(0.5)).distance(at).total_cmp(&(b.as_vec3() + Vec3::splat(0.5)).distance(at)))
        })
    }

    /// Moves one creature's body a tick along its decision.
    pub(crate) fn move_creature(&mut self, id: u32, dt: f32) {
        let Some(creature) = self.creatures.live.get(&id) else { return };
        let species = self.content.creatures.get(creature.species);
        let mut body = creature.body;
        let mut decision = creature.decision;
        let held = self.tick < creature.held_until;
        // A land animal out of its depth makes for the nearest shore, whatever it had in mind
        // (rather than paddle where its wander or the food it was after takes it).
        // Once making for it, it keeps on through the shallows until it is out of the water.
        let land = !held && species.def.habitat.water == Water::Land && body.in_water;
        let mut looked_for_shore = false;
        // Out of its depth is two blocks of water under it: bobbing at the top of a ford one
        // deep, its feet a little over the water, it is still in its depth.
        let surface = (body.position + Vec3::Y * 0.5).floor().as_ivec3();
        let deep = land && (1..=2).all(|down| Ground::of(self).water_at(surface - IVec3::Y * down));
        let shore = if !deep && !(land && creature.shore.is_some()) {
            None
        } else if creature.shore_at.is_some_and(|at| self.tick < at + TICKS_PER_SECOND as u64) {
            // Looked within the second, found or not (far out in a lake there is none).
            creature.shore
        } else {
            looked_for_shore = true;
            self.shore_for(body.position, body.size.1.ceil() as i32)
        };
        if let Some(cell) = shore {
            decision.goal = Some(cell.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
            decision.speed = decision.speed.max(species.def.movement.walk);
        }
        // Winding up, it still turns and walks at its target until its aim locks; charging,
        // it runs straight on.
        let charging = creature.charging;
        let tracking = if held { self.tracking_drive(id) } else { None };
        let drive = match decision.goal {
            _ if charging.is_some() => charging.map(|c| c.drive(creature.scale)).unwrap_or_default(),
            _ if held => tracking.unwrap_or_default(),
            Some(goal) if goal.with_y(0.0).distance(body.position.with_y(0.0)) > 0.4 => {
                let wish = (goal - body.position).with_y(if species.def.habitat.water == Water::Swim { goal.y - body.position.y } else { 0.0 });
                Drive { wish: wish.normalize_or_zero(), speed: decision.speed * (1.0 - self.fights.slow(id, self.game_secs())), face: decision.face.map(|f| f - body.position) }
            }
            _ => Drive { wish: Vec3::ZERO, speed: 0.0, face: decision.face.map(|f| f - body.position) },
        };
        if held && tracking.is_none() && charging.is_none() {
            // Held still: nothing carries it along, neither its legs nor a neighbour's push.
            body.velocity.x = 0.0;
            body.velocity.z = 0.0;
        }
        let before = body.position;
        body.reach = species.reach * creature.scale;
        body.head = species.head * creature.scale;
        // Going where it chose, it finds its way round a wall, a corner or a tree in the way
        // rather than pressing into it: a creature run from beside a wall runs along it.
        // Where that is not enough (its straight way was shut), it looks for a short way
        // round (`path::find`) and follows it: the door in the wall, the gap in the fence.
        let (movement, water) = (&species.def.movement, species.def.habitat.water);
        // A boss, or a wild fighter on a chase, climbs a railing (`Ground::climbing`); a
        // kept animal, even defending itself, stays in its railed pen.
        let climbs = species.def.boss || creature.owner.is_none() && creature.mind.doing.is_some_and(|i| matches!(species.modules[i].0.name(), "hunt" | "defend"));
        // The way it follows after this tick, and the goal cell and tick of a fresh search.
        let (mut way, mut searched) = (Vec::new(), None);
        // What it wanted before steering: a creature boxed in steers to a standstill but still
        // wants to go, so its stuck clock runs and it gives the goal up.
        let wanted = drive.wish;
        let (drive, detour) = match decision.goal {
            Some(goal) if !held && drive.wish != Vec3::ZERO && motion::walks(movement, water, drive) => {
                let to = goal.floor().as_ivec3();
                let mut cells = creature.way.clone();
                // A way followed to its end is done with: meeting the wall again, it looks again.
                let stale = cells.is_empty() || creature.way_to.is_none_or(|known| known.as_vec3().distance(to.as_vec3()) > 2.0);
                // Going round, pressing or boxed in for half a second, or on a way whose
                // goal moved. A search that found nothing is tried again a second on, from
                // wherever it has got to.
                let going_round = creature.detour != 0.0 || creature.stuck > 0.5;
                // Its way is of squares as wide as it is, and it makes for each one's middle.
                let span = path::Walker::span_of(body.size.0);
                let walker = path::Walker { height: body.size.1.ceil() as i32, climbs: movement.jump > 0.0, wary: water == Water::Land, span };
                if (going_round || !cells.is_empty()) && stale && self.tick >= creature.way_at + TICKS_PER_SECOND as u64 {
                    let found = path::find(&Ground::climbing(self, climbs), &walker, walker.corner(body.position + Vec3::Y * 0.01), walker.corner(goal));
                    #[cfg(test)]
                    tick_cost_tests::SEARCHES.with(|n| n.set((n.get().0 + 1, n.get().1 + found.is_none() as u64)));
                    searched = Some((found.is_some().then_some(to), self.tick));
                    cells = found.unwrap_or_default();
                }
                // At a cell (beside it, at its level), or already nearer the one after: on to
                // the next, so it never turns back for a cell it brushed past.
                let off = |cell: &IVec3| (walker.middle(*cell) - body.position).with_y(0.0).length();
                // Whether its box goes straight to the middle of `cell`, a cell at its level,
                // meeting nothing: through a doorway barely wider than it, only once lined up
                // with it.
                let ground = Ground::climbing(self, climbs);
                let straight = |cell: &IVec3| goes_straight(body.aabb(), (walker.middle(*cell) - body.position).with_y(0.0), |p| motion::Surroundings::solid(&ground, p));
                while let Some(cell) = cells.first() {
                    // Level with it: a cell a block up is not reached until it has climbed.
                    // In water its feet bob at the surface, a block over a ford's cells.
                    // On land, a cell a block down is not reached from its lip.
                    let level = (if body.in_water { -1.2 } else { -0.6 }..0.6).contains(&(cell.y as f32 - body.position.y));
                    // On to the next only where it goes straight there: else it makes for this
                    // cell's middle first, lining up with the gap ahead (a boar at a doorway a
                    // block across), rather than cut the corner into its frame and dither
                    // there. At the middle it goes on whatever.
                    let on = || cells.get(1).is_none_or(|after| after.y != cell.y || straight(after));
                    // Past the middle on the line to the next cell by no more than a step
                    // takes it, and lined up with that line (a run overshooting the middle),
                    // it goes on rather than circle back for it.
                    let past = cells.get(1).is_some_and(|after| {
                        let along = (walker.middle(*after) - walker.middle(*cell)).with_y(0.0).normalize_or_zero();
                        let off = (body.position - walker.middle(*cell)).with_y(0.0);
                        (0.0..0.1 + drive.speed * dt).contains(&off.dot(along)) && (off - along * off.dot(along)).length() < 0.1
                    });
                    if !(level && (off(cell) < 0.1 || past || ((off(cell) < 0.6 || cells.get(1).is_some_and(|after| off(after) < 1.0)) && on()))) {
                        break;
                    }
                    cells.remove(0);
                }
                let wish = cells.first().map_or(drive.wish, |cell| (walker.middle(*cell) - body.position).with_y(0.0).normalize_or_zero());
                let (heading, side) = motion::steer(&body, &Ground::climbing(self, climbs), movement, water, wish, creature.detour, drive.speed, dt);
                way = cells;
                (Drive { wish: heading, ..drive }, side)
            }
            _ => (drive, 0.0),
        };
        motion::step(&mut body, &Ground::climbing(self, climbs), movement, water, drive, dt);
        let creature = self.creatures.live.get_mut(&id).expect("still there");
        // What a search found holds for a second, bobbing at the surface or not; out on dry
        // ground it is done with.
        if looked_for_shore {
            (creature.shore, creature.shore_at) = (shore, Some(self.tick));
        } else if body.on_ground && !body.in_water {
            (creature.shore, creature.shore_at) = (None, None);
        }
        creature.detour = detour;
        creature.way = way;
        if let Some((to, at)) = searched {
            (creature.way_to, creature.way_at) = (to, at);
            // No way there at all (a player shut in a box, or behind a fence with no gap):
            // going round on, it gives the goal up sooner.
            if to.is_none() {
                creature.stuck = creature.stuck.max(NO_WAY_STUCK);
            }
        }
        let (mut balked, mut no_way) = (false, false);
        // Its last look for a way, lately, found none.
        let no_way_lately = creature.way_to.is_none() && creature.way_at != 0 && self.tick < creature.way_at + 3 * TICKS_PER_SECOND as u64;
        // Stuck against something for two seconds, or going round it for four: give up the
        // goal.
        let gained = body.position.distance(before) >= drive.speed * dt * 0.2;
        if wanted != Vec3::ZERO && (!gained || detour != 0.0) {
            creature.stuck += if gained { dt * 0.5 } else { dt };
            if creature.stuck > 2.0 {
                creature.decision.goal = None;
                creature.mind.goal = None;
                creature.stuck = 0.0;
                // Balked twice running in a chase (a player behind a fence it has no way
                // through, or round a box it cannot get into): it gives the chase up and
                // goes home rather than hang there. Back at it within a minute, it gives up
                // at the first balk.
                let chasing = creature.mind.doing.is_some_and(|i| matches!(species.modules[i].0.name(), "hunt" | "defend"));
                if chasing && !species.def.boss {
                    let again = self.tick < creature.balked_at + BALK_WINDOW;
                    creature.balked = if again { creature.balked + 1 } else { 1 };
                    creature.balked_at = self.tick;
                    let lately = creature.gave_up_at.is_some_and(|at| self.tick < at + REBALK_WINDOW);
                    if creature.balked >= if lately { 1 } else { 2 } {
                        creature.balked = 0;
                        creature.gave_up_at = Some(self.tick);
                        balked = true;
                        no_way = no_way_lately;
                    }
                }
            }
        } else if !no_way_lately {
            // A tick's headway does not clear the clock of one that lately found no way:
            // jostled along the wall in a doorway too narrow for it, it is getting nowhere.
            creature.stuck = 0.0;
        }
        if body != creature.body {
            creature.body = body;
            creature.changed = true;
        }
        let handoff = self.overworld.settlement_people().live.handoff;
        if balked && self.villagers.errand(id, handoff).is_none() && self.wandering.errand(id).is_none() {
            // With no way to its target at all, struck again soon it runs rather than come
            // back to the fence (`Memory::NoWay`).
            let mind = &mut self.creatures.live.get_mut(&id).expect("still there").mind;
            if no_way { mind.find_no_way() } else { mind.give_up() }
        }
        if charging.is_some() {
            self.charge_ran(id, before);
        }
        let Some(body) = self.creatures.live.get(&id).map(|c| c.body) else { return };
        if body.position.y < -64.0 {
            self.creatures.live.remove(&id);
            self.creatures.gone.push(CreatureGone { id, died: false });
        }
    }

    /// Acts on what a creature decided.
    fn carry_out(&mut self, id: u32, decision: Decision, neighbours: &Neighbours) {
        let Some(creature) = self.creatures.live.get(&id) else { return };
        let species_id = creature.species;
        let position = creature.body.position;
        if let Some(player) = decision.attack {
            // It winds up first; the blow lands after (`fight`).
            self.wind_up(id, player);
        }
        if let Some(cell) = decision.eat.filter(|&cell| self.is_trough(cell)) {
            self.eat_from_trough(id, cell);
        } else if let Some(cell) = decision.eat {
            let day = self.day;
            if let Some(creature) = self.creatures.live.get_mut(&id) {
                creature.fed = day;
            }
            // Grazing wears the land: now and then a tuft goes for good.
            // By the world's count of rolls, never the tick it grazes on.
            let roll = self.meta.next_roll(crate::save::ROLL_GRAZE)[0];
            let block = self.block(cell);
            if roll < 0.3 && block.is_some_and(|b| self.content.blocks.info(b).replaceable && !self.content.blocks.info(b).solid) {
                self.set_block(cell, BlockId::AIR);
            }
        }
        if let Some(to) = decision.blink {
            let height = self.content.creatures.get(species_id).def.body.size;
            let landing = (0..6).map(|dy| (to + Vec3::Y * 3.0 - Vec3::Y * dy as f32).floor()).find(|cell| {
                let feet = cell.as_ivec3();
                // On footing: never on a railing's narrow top.
                self.is_solid(feet - IVec3::Y) && !Ground::of(self).railing(feet - IVec3::Y) && !Aabb::standing(cell.as_ivec3().as_vec3() + Vec3::new(0.5, 0.0, 0.5), height.0, height.1).intersects(|p| self.is_solid(p))
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
            for other in neighbours.kind_around(position, species_id, id) {
                if let Some(kin) = self.creatures.live.get_mut(&other)
                    && kin.species == species_id
                    && kin.body.position.distance(position) < KIN_RANGE
                {
                    kin.mind.alarmed();
                }
            }
        }
    }

    /// A creature's blow comes down on a player (`fight`): a hit rolled from its level
    /// against the player's Defence, its damage times `share` (what a guard lets through).
    /// Returns the damage, or `None` for a miss, which may train Defence (`defence_xp`).
    pub(crate) fn creature_strikes(&mut self, creature: u32, species: CreatureId, player: PlayerId, share: f32) -> Option<u16> {
        let creature_def = self.content.creatures.get(species);
        let def = &creature_def.def;
        let attack = creature_def.attack_at(self.creatures.live.get(&creature).map_or(0, |c| c.attack))?;
        let target = self.players.get_mut(&player)?;
        if !target.health.alive() || target.allow_flight {
            return Some(0);
        }
        // By the count of blows that have come down on them, not the tick it falls on.
        let [hit, amount] = target.draw(Draw::Blow);
        let target = &self.players[&player];
        let defence_skill = self.skill("core:defence");
        // A Defence boost (a prayer) raises the level the blow is rolled against.
        let book = &self.content.prayers;
        let boost = 1.0 + book.boost(&target.praying.on, voxmar_engine::prayer::Stat::Defence);
        let defence = voxmar_engine::combat::by_chance(self.level(player, defence_skill) as f32 * boost, unit_from(mix(hit.to_bits() as u64, 0xdef))).min(u8::MAX as u16) as u8;
        // A ward up against the blow's style stops its share; with a guard, the larger stop.
        // A boss's blows by a smaller share (`boss_ward`).
        let ward = if def.boss { book.rules.boss_ward } else { book.rules.ward };
        let share = if book.warded(&target.praying.on, attack.style) { share.min(1.0 - ward) } else { share };
        let last_breath = book.last_breath(&target.praying.on);
        let (low, high) = attack.damage;
        // Worn armour adds to the defence roll, by the wearer's own Defence level.
        let armour = self.content.combat.armour_of(target.held(), &target.inventory.worn, attack.style, |skill| self.content.skills.level(target.skills.xp(skill)));
        let accuracy = attack.accuracy.unwrap_or((self.content.combat.feel.creature_accuracy * def.level as f32).round() as u16);
        if hit >= voxmar_engine::combat::hit_chance(def.level, accuracy, defence, armour) {
            self.defence_xp(player, creature, species);
            return None;
        }
        let rolled = low + ((amount * (high - low + 1) as f32) as u8).min(high - low);
        let damage = voxmar_engine::combat::scaled(rolled as u16, share, unit_from(mix(amount.to_bits() as u64, 0x6a4d))) as u8;
        // In a bout, no blow takes the player below the share they spar down to.
        let damage = match self.bout_floor(creature, player) {
            Some(down_to) => {
                let health = &self.players[&player].health;
                damage.min(health.current.saturating_sub((health.max as f32 * down_to).ceil() as u8))
            }
            None => damage,
        };
        let tick = self.tick;
        let target = self.players.get_mut(&player).expect("checked above");
        if target.wound(damage, tick) {
            target.connection.send(ServerMessage::Died);
        }
        // Last Breath: left under its share of health, a share comes back, and every prayer
        // point goes.
        if let Some((below, heal)) = last_breath.filter(|_| target.health.alive()) {
            let most = target.health.max as f32;
            if (target.health.current as f32) < below * most {
                target.health.heal(((heal * most).round() as u8).max(1));
                target.praying.points = 0.0;
                target.praying.on.clear();
            }
        }
        target.changed(traits::HEALTH);
        Some(damage as u16)
    }

    /// Experience in Defence for creature `id`'s blow turned aside (missed by the defence
    /// roll, or perfectly blocked): four a point of the damage it would do, only while the
    /// player fights it back, having landed a damaging blow on it lately, and it is a match
    /// for them (`DefenceTraining`, design wiki draft 63).
    pub(crate) fn defence_xp(&mut self, player: PlayerId, id: u32, species: CreatureId) {
        if self.villager_calm_for(id, player) {
            return;
        }
        let creature = self.content.creatures.get(species);
        let def = &creature.def;
        let Some(attack) = creature.attack_at(self.creatures.live.get(&id).map_or(0, |c| c.attack)) else { return };
        let Some(skill) = self.skill("core:defence") else { return };
        let (low, high) = attack.damage;
        // Nothing for turning aside what could never hurt.
        if high == 0 {
            return;
        }
        let defence = self.level(player, Some(skill));
        let rules = self.content.creatures.defence();
        let window = (rules.fought_within * TICKS_PER_SECOND as f32) as u64;
        let fought = self.creatures.live.get(&id).and_then(|c| c.fought);
        let fighting = fought.is_some_and(|(by, at)| by == player && self.tick.saturating_sub(at) <= window);
        if !(fighting && rules.matches(def.level, defence)) {
            return;
        }
        // By the species' share: a villager teaches nothing, a guard does.
        let tenths = ((40 * (low as u32 + high as u32) / 2).max(10) as f32 * def.xp.combat).round() as u32;
        if tenths == 0 {
            return;
        }
        let Some(target) = self.players.get_mut(&player) else { return };
        target.skills.grant(skill, tenths);
        target.changed(traits::SKILLS);
        target.connection.send(ServerMessage::Xp { skill, tenths });
    }

    /// A player strikes a creature (`core:attack`): the swing starts, or waits for the
    /// beat, and its blow lands when it has wound up (`fight`).
    pub(crate) fn player_strikes(&mut self, player: PlayerId, id: u32) {
        self.swing_at(player, id);
    }

    /// A player's blow lands on a creature, with what they hold (`combat`): a hit rolled
    /// from the weapon's skill and accuracy against the creature's defence (half against
    /// the style it is weak to), damage up to the weapon's max hit for their level, as the
    /// stroke raises them. Returns the damage, 0 for a miss, `None` if it is gone.
    pub(crate) fn land_blow(&mut self, player: PlayerId, id: u32, weapon: &voxmar_engine::combat::Weapon, stroke: voxmar_engine::combat::Stroke) -> Option<u16> {
        let calm = self.villager_struck(id, player);
        let creature = self.creatures.live.get(&id)?;
        let species = self.content.creatures.get(creature.species);
        // Accuracy and Strength boosts (prayers) raise the level a hit is rolled with and
        // the max hit.
        let on = self.players.get(&player).map_or(&[][..], |p| &p.praying.on[..]);
        let book = &self.content.prayers;
        let (accurate, strong) = (1.0 + book.boost(on, voxmar_engine::prayer::Stat::Accuracy), 1.0 + book.boost(on, voxmar_engine::prayer::Stat::Strength));
        let level = self.level(player, weapon.skill);
        let sunder = self.fights.sunder(id, self.game_secs());
        let defence = voxmar_engine::combat::defence_against(species.def.defence, species.def.weak, weapon.style);
        // By the count of blows they have struck, not the tick it lands on.
        let [hit, amount] = self.players.get_mut(&player)?.draw(Draw::Strike);
        // Shares of a level or a max hit round by chance, so a small one still counts.
        let defence = voxmar_engine::combat::by_chance(defence as f32 * (1.0 - sunder), unit_from(mix(hit.to_bits() as u64, 0x5d)))
            .min(u8::MAX as u16) as u8;
        let attack = voxmar_engine::combat::by_chance(level as f32 * accurate, unit_from(mix(hit.to_bits() as u64, 0xacc))).min(u8::MAX as u16) as u8;
        if !stroke.certain && hit >= voxmar_engine::combat::hit_chance_with(attack, weapon.accuracy, defence, 0, stroke.accuracy) {
            let creature = self.creatures.live.get_mut(&id).expect("checked above");
            if !calm {
                creature.mind.hurt_young(species, Some(player), creature.health as f32 / species.def.health as f32, creature.grows_up.is_some());
            }
            self.struck(player, id, 0, false, stroke.blow);
            return Some(0);
        }
        let max_hit = voxmar_engine::combat::by_chance(weapon.max_hit(level) as f32 * strong, unit_from(mix(amount.to_bits() as u64, 0x57e)));
        let max_hit = stroke.max_hit(max_hit, unit_from(mix(amount.to_bits() as u64, 0x3a7)));
        let full = (1 + (amount * max_hit as f32) as u16).min(max_hit);
        // A blow struck sooner than the buffer lands lighter (`combat`).
        let damage = voxmar_engine::combat::scaled(full, stroke.strength, unit_from(mix(amount.to_bits() as u64, 0x5e1f)));
        self.hurt_creature(player, id, damage, weapon.skill, stroke.blow)
    }

    /// `damage` to a creature from a player's blow (or a bleed), with the experience it
    /// gives: four a point in the weapon's skill, a third of that in Hitpoints
    /// (RuneScape). Returns the damage done, `None` if it is gone.
    pub(crate) fn hurt_creature(&mut self, player: PlayerId, id: u32, damage: u16, skill: Option<SkillId>, blow: voxmar_engine::combat::Blow) -> Option<u16> {
        let calm = self.villager_struck(id, player);
        let bout = self.bout_floor(id, player);
        let creature = self.creatures.live.get_mut(&id)?;
        let species = self.content.creatures.get(creature.species);
        // No knockback: the creature stays where the blow found it (its model flinches).
        creature.changed = true;
        // In a bout, no blow takes it below the share it spars down to.
        let floor = bout.map_or(0, |down_to| (species.def.health as f32 * down_to).ceil() as u16);
        let damage = damage.min(creature.health.saturating_sub(floor));
        creature.health -= damage;
        creature.hurt_by = Some(player);
        // A blow that hurt it keeps the fight on, for Defence (`creature_strikes`).
        if damage > 0 {
            creature.fought = Some((player, self.tick));
        }
        if !calm {
            creature.mind.hurt_young(species, Some(player), creature.health as f32 / species.def.health as f32, creature.grows_up.is_some());
            // A blow, not a bleed, keeps a hunter past its leash fighting (`Memory::Struck`).
            if !matches!(blow, voxmar_engine::combat::Blow::Bleed) {
                creature.mind.remember(brain::Memory::Struck, brain::STRUCK_LATELY);
            }
        }
        // RuneScape: four experience a point of damage in the style's skill, a third of
        // that in Hitpoints; by the species' share.
        let share = |tenths: u32| if calm { 0 } else { (tenths as f32 * species.def.xp.combat).round() as u32 };
        let grants = [(skill, share(40 * damage as u32)), (self.skill("core:hitpoints"), share(13 * damage as u32))];
        if let Some(target) = self.players.get_mut(&player) {
            for (skill, tenths) in grants {
                if let Some(skill) = skill.filter(|_| tenths > 0) {
                    target.skills.grant(skill, tenths);
                    target.connection.send(ServerMessage::Xp { skill, tenths });
                }
            }
            target.changed(traits::SKILLS);
        }
        let killed = self.creatures.live[&id].health == 0;
        if !killed {
            self.phase_check(id);
        }
        self.wandering_hurt(id, player);
        self.struck(player, id, damage, killed, blow);
        if killed {
            self.creature_dies(id);
        }
        Some(damage)
    }

    /// A creature hurts itself (a fall its charge ran off): no player's blow, so no player
    /// is shown a hit or earns experience for it. Killed so, it dies as one felled by no
    /// hand, its loot for the player who hurt it lately (as long as it remembers who hurt
    /// it, `brain::HURT_REMEMBERED`); hurt by nobody lately, it dies with nothing.
    pub(crate) fn fall_hurt_creature(&mut self, id: u32, damage: u16) {
        let tick = self.tick;
        let Some(creature) = self.creatures.live.get_mut(&id) else { return };
        let damage = damage.min(creature.health);
        creature.health -= damage;
        creature.changed = true;
        let lately = (brain::HURT_REMEMBERED * TICKS_PER_SECOND as f32) as u64;
        let last = creature.fought.filter(|&(_, at)| tick.saturating_sub(at) <= lately).map(|(by, _)| by);
        let killed = creature.health == 0;
        if killed {
            self.creature_felled(id, last);
        } else if damage > 0 {
            self.phase_check(id);
        }
    }

    /// A creature dies: its loot pops out, its killer earns experience.
    pub(crate) fn creature_dies(&mut self, id: u32) {
        self.creature_dies_by(id, true);
    }

    /// A creature dies: its loot pops out for the player who last hurt it, as theirs
    /// (their tool, their skill, their journal), and with `xp` they earn the kill's experience.
    fn creature_dies_by(&mut self, id: u32, xp: bool) {
        let Some(creature) = self.creatures.live.remove(&id) else { return };
        self.creatures.gone.push(CreatureGone { id, died: true });
        // One of a settlement's people: noted dead now, so no save misses it.
        self.villager_died(id);
        self.wandering_died(id);
        self.lair_boss_died(id);
        let species = self.content.creatures.get(creature.species);
        let killer = creature.hurt_by.and_then(|p| self.players.get(&p));
        let tool: Vec<String> = killer.and_then(|p| p.held()).map_or_else(Vec::new, |item| self.content.blocks.def(item).tags.clone());
        let bonus = match (killer, species.xp_skill) {
            (Some(p), Some(skill)) => level_bonus(self.content.skills.level(p.skills.xp(skill)), 1, self.content.skills.max_level()),
            _ => 0.0,
        };
        let moment = Moment { bonus, tool: &tool, night: self.daylight() < 0.3, by_player: killer.is_some() };
        // Each roll numbered by the killer's own count of kills, or the world's count of loot
        // rolls for a death nobody dealt; never by the tick it dies on.
        let by = creature.hurt_by.filter(|p| self.players.contains_key(p));
        let (players, meta) = (&mut self.players, &mut self.meta);
        let found = loot::roll(&species.loot, &moment, || match by.and_then(|p| players.get_mut(&p)) {
            Some(player) => player.draw(crate::save::Draw::Kill)[0],
            None => meta.next_loot(),
        });
        let cell = creature.body.position.floor().as_ivec3();
        // The killer's journal: one more of the species (people are not counted), and each
        // drop for the collection log.
        let mut journal: Vec<_> = Self::page_of(&species.def).map(|page| (page, species.def.name.clone(), 1)).into_iter().collect();
        journal.extend(found.iter().map(|&(item, count)| (voxmar_engine::journal::Page::Collection, self.content.blocks.def(item).name.clone(), count as u32)));
        // Its loot comes out once its body has fallen, never from an animal still standing.
        let due = self.tick + (species.def.death.drops_after() * TICKS_PER_SECOND as f32).round() as u64;
        for (i, (item, count)) in found.into_iter().enumerate() {
            let body = dropped::popped(cell, self.meta.next_pop(cell, i as u32 + id));
            self.creatures.falling_loot.push((due, ItemStack::new(item, count), body));
        }
        if let (Some(player), Some(skill), true) = (creature.hurt_by, species.xp_skill, xp) {
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

    /// A creature brought down by no player's hand (a hunting party's quarry): it teaches
    /// nobody anything. With `shared`, the player who drew first blood is given their share
    /// of the kill: its loot drops for them as if theirs, and goes in their journal; else it
    /// dies with nothing.
    pub(crate) fn creature_felled(&mut self, id: u32, shared: Option<PlayerId>) {
        if let Some(player) = shared {
            if let Some(creature) = self.creatures.live.get_mut(&id) {
                creature.hurt_by = Some(player);
            }
            self.creature_dies_by(id, false);
            return;
        }
        if self.creatures.live.remove(&id).is_some() {
            self.creatures.gone.push(CreatureGone { id, died: true });
            self.villager_died(id);
            self.wandering_died(id);
            self.lair_boss_died(id);
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
        let seed = mix(mix(self.meta.seed, self.creatures.next as u64), salt);
        self.spawn_seeded(species, at, seed)
    }

    /// Puts a creature whose looks and mind come from `seed`, so a settlement's
    /// resident remains the same individual on each visit.
    pub(crate) fn spawn_seeded(&mut self, species: CreatureId, at: Vec3, seed: u64) -> Option<u32> {
        self.spawn_seeded_unseen(species, at, seed, 0.0)
    }

    /// Starts creature `id`'s own count of rolls (`Live::rolls`) at the world's next roll
    /// (`ROLL_START`): one with a fixed seed (a lair's boss, a settlement's resident), woken
    /// again as the same individual, draws its thoughts afresh rather than over again.
    pub(crate) fn start_rolls(&mut self, id: u32) {
        let [a, b] = self.meta.next_cast(crate::save::ROLL_START);
        if let Some(creature) = self.creatures.live.get_mut(&id) {
            creature.rolls = u64::from(a.to_bits()) << 32 | u64::from(b.to_bits());
        }
    }

    /// Natural spawns check the individual's gene-scaled body before it enters the
    /// world. Explicit summons and seeded residents may appear in view.
    fn spawn_unseen(&mut self, species: CreatureId, at: Vec3, salt: u64, angle: f32) -> Option<u32> {
        let seed = mix(mix(self.meta.seed, self.creatures.next as u64), salt);
        self.spawn_seeded_unseen(species, at, seed, angle)
    }

    fn spawn_seeded_unseen(&mut self, species: CreatureId, at: Vec3, seed: u64, angle: f32) -> Option<u32> {
        // Settlements' people and the land's animals each have a budget; the people and
        // animals of wandering parties (`wandering`) have their own, so they use up
        // neither.
        let people = self.creatures.live.keys().filter(|id| self.content.creatures.get(self.creatures.live[*id].species).def.kind == Kind::Villager);
        let (all, settled) = people.fold((0, 0), |(all, settled), id| (all + 1, settled + usize::from(!self.creatures.live[id].wanderer)));
        let full = if self.content.creatures.get(species).def.kind == Kind::Villager {
            settled >= self.overworld.settlement_people().live.most
        } else {
            self.creatures.live.len() - all >= self.content.creatures.limits().most
        };
        if full {
            return None;
        }
        self.place_creature(species, at, seed, angle)
    }

    /// Puts a creature of `species` at `at` whose looks and mind come from `seed`, if its
    /// body is not seen appearing; the caller has judged whether the world has room.
    pub(crate) fn place_creature(&mut self, species: CreatureId, at: Vec3, seed: u64, angle: f32) -> Option<u32> {
        let id = self.creatures.next;
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
        // Facing open air: a head spawned into a trunk or a wall would show in it.
        let looks = self.content.creatures.get(species);
        let yaw = (0..8)
            .map(|i| yaw + i as f32 * std::f32::consts::FRAC_PI_4)
            .find(|&yaw| !Body { reach: looks.reach * scale, head: looks.head * scale, ..Body::new(at, yaw, size) }.head_blocked(&Ground::of(self)))
            .unwrap_or(yaw);
        if self.model_seen_by_a_player(species, at, yaw, scale, angle) {
            return None;
        }
        self.creatures.next += 1;
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
            rolls: 0,
            hurt_by: None,
            fought: None,
            was_kept: false,
            lured: false,
            wanderer: false,
            companion: false,
            stuck: 0.0,
            detour: 0.0,
            way: Vec::new(),
            way_to: None,
            shore: None,
            shore_at: None,
            balked: 0,
            balked_at: 0,
            gave_up_at: None,
            way_at: 0,
            owner: None,
            owner_id: None,
            trust: 0.0,
            trusting: None,
            ready_until: 0,
            held_until: 0,
            winding: false,
            tracking: None,
            attack: 0,
            phase: 0,
            rearing: false,
            charging: None,
            staggered: false,
            mended: 0,
            genome,
            traits,
            breeds_again: 0,
            grows_up: None,
            adult_scale: scale,
            products_due: def.products.iter().enumerate().map(|(i, p)| self.tick + product_ticks(p.every, seed, i)).collect(),
            condition: self.content.creatures.condition().tamed,
            reckoned: self.day,
            fed: self.day,
            needs: Needs::default(),
            climate_fit: 1.0,
            sent: None,
            changed: true,
        };
        // Where it arrived, for a trap it runs through before the traps next look.
        self.catches.arrived(id, creature.body.position);
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

    /// Most monsters near a player now: a new world's first night brings only a few, until
    /// its first dawn: the world begins at sunrise, so that is the first day, and any
    /// darkness that runs on past its end.
    pub(crate) fn monsters_cap(&self) -> usize {
        let limits = self.content.creatures.limits();
        if first_night(self.day, self.daylight()) { limits.monsters_near.min(limits.first_night_monsters) } else { limits.monsters_near }
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
        let nearby = |of: Budget| {
            self.creatures
                .live
                .values()
                .filter(|c| c.body.position.distance(at) < NEAR && budget(&self.content.creatures.get(c.species).def) == Some(of))
                .count()
        };
        let (animals, monsters, birds) = (nearby(Budget::Animal), nearby(Budget::Monster), nearby(Budget::Bird));
        // A new world's first night brings only a few, until its first dawn: the world begins
        // at sunrise, so that is the first day, and any darkness that runs on past its end.
        let monsters_cap = self.monsters_cap();
        let room_of = |of: Budget| match of {
            Budget::Animal => (animals, limits.animals_near),
            Budget::Bird => (birds, limits.birds_near),
            Budget::Monster => (monsters, monsters_cap),
        };
        if [Budget::Animal, Budget::Bird, Budget::Monster].into_iter().all(|of| room_of(of).0 >= room_of(of).1) {
            return;
        }
        // By the world's count of rolls, never the tick it is tried on.
        let seed = mix(self.meta.roll_seed(crate::save::ROLL_SPAWN), salt ^ 0x5ba7);
        let angle = unit_from(seed) * std::f32::consts::TAU;
        let distance = limits.spawn_near + unit_from(mix(seed, 1)) * (limits.spawn_far - limits.spawn_near);
        let x = (at.x + angle.cos() * distance).floor() as i32;
        let z = (at.z + angle.sin() * distance).floor() as i32;
        if !self.world.contains(ColumnPos::of_block(IVec3::new(x, 0, z))) {
            return;
        }
        let Some(spot) = self.standing_spot(x, z, at.y.floor() as i32) else { return };
        if self.seen_by_a_player(spot, limits.spawn_unseen_angle) || self.in_lair_quiet(spot) {
            return;
        }
        let site = self.overworld.site(x, z);
        let biome = self.overworld.biomes().get(site.biome);
        let underground = spot.y < site.height - 4;
        let dark = !self.lit_near(spot) && (underground || self.daylight() < 0.25);
        let in_water = Ground::of(self).water_at(spot);
        let daylight = self.daylight();
        let mut choices: Vec<(CreatureId, f32)> = Vec::new();
        for (id, species) in self.content.creatures.iter() {
            let def = &species.def;
            // People come with their settlements (`villagers`), never from the land.
            if def.kind == Kind::Villager || def.habitat.party_only {
                continue;
            }
            let monster = def.kind == Kind::Monster;
            let (here, cap) = room_of(budget(def).expect("not a villager"));
            if here >= cap || (monster && !dark) {
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
        // Birds are drawn apart from what walks, with a draw of their own at the same spot, so
        // they add to the animals about instead of taking their places.
        let (birds, ground): (Vec<_>, Vec<_>) = choices.into_iter().partition(|&(id, _)| budget(&self.content.creatures.get(id).def) == Some(Budget::Bird));
        for (choices, seed) in [(ground, seed), (birds, mix(seed, 0xb1d5))] {
            if self.wild_room() == 0 {
                break;
            }
            // Sometimes nothing: some spots stay empty, so there are gaps between herds.
            let total: f32 = choices.iter().map(|c| c.1).sum();
            let nothing = if choices.iter().all(|&(id, _)| self.content.creatures.get(id).def.kind == Kind::Monster) { limits.empty_for_monsters } else { limits.empty_for_animals };
            let mut pick = unit_from(mix(seed, 2)) * (total + nothing);
            let Some(&(species, _)) = choices.iter().find(|c| {
                pick -= c.1;
                pick <= 0.0
            }) else {
                continue;
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
            let def = &self.content.creatures.get(species).def;
            let (monster, of) = (def.kind == Kind::Monster, budget(def).expect("not a villager"));
            let (here, cap) = room_of(of);
            let room = cap.saturating_sub(here);
            let count = count.min(room.max(1).min(self.wild_room()).min(u8::MAX as usize) as u8);
            for i in 0..count.max(1) {
                let offset = if i == 0 { IVec3::ZERO } else { IVec3::new((mix(seed, 10 + i as u64) % 7) as i32 - 3, 0, (mix(seed, 20 + i as u64) % 7) as i32 - 3) };
                let Some(cell) = self.standing_spot(spot.x + offset.x, spot.z + offset.z, spot.y) else { continue };
                if (monster && self.lit_near(cell)) || self.seen_by_a_player(cell, limits.spawn_unseen_angle) {
                    continue;
                }
                let position = cell.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
                if !self.nearby_room(position, of, cap) {
                    continue;
                }
                self.spawn_unseen(species, position, seed ^ i as u64, limits.spawn_unseen_angle);
            }
        }
    }

    /// A bounded admin inspection batch through the ordinary natural spawning path.
    /// Hold admitted births so their initial poses can be inspected after replication.
    pub(crate) fn populate_near(&mut self, player: PlayerId) -> usize {
        let began = std::time::Instant::now();
        let first = self.creatures.next;
        for salt in 0..128 {
            self.spawn_near(player, salt);
        }
        let mut count = 0;
        for (_, creature) in self.creatures.live.iter_mut().filter(|(id, _)| **id >= first) {
            creature.held_until = self.tick + 60 * TICKS_PER_SECOND as u64;
            count += 1;
        }
        tracing::info!("natural population: {count} births from 128 attempts in {:.3} ms", began.elapsed().as_secs_f64() * 1000.0);
        count
    }

    /// Every joined player's nearby budget applies, including where two players' areas
    /// overlap. Count accepted members again so a group cannot cross either cap.
    pub(crate) fn nearby_room(&self, at: Vec3, of: Budget, cap: usize) -> bool {
        self.players.values().filter(|p| p.joined && p.position.distance(at) < NEAR).all(|player| {
            self.creatures
                .live
                .values()
                .filter(|c| c.body.position.distance(player.position) < NEAR && budget(&self.content.creatures.get(c.species).def) == Some(of))
                .count()
                < cap
        })
    }

    /// Whether a player could see something appear at `spot`: it lies within `angle`
    /// degrees of where they look, within sight range, and nothing opaque stands between
    /// their eye and it.
    pub(crate) fn seen_by_a_player(&self, spot: IVec3, angle: f32) -> bool {
        self.points_seen_by_a_player(&[spot.as_vec3() + Vec3::new(0.5, 1.0, 0.5)], angle)
    }

    /// Check each rendered part's centre and corners in the individual's initial pose.
    /// Collision boxes omit ears, wings and tails: those must not pop into view either.
    /// Parts are already cached at the species' model scale; apply individual scale once.
    fn model_seen_by_a_player(&self, species: CreatureId, at: Vec3, yaw: f32, scale: f32, angle: f32) -> bool {
        if angle == 0.0 {
            return false;
        }
        let turn = Quat::from_rotation_y(yaw);
        self.content.creatures.get(species).parts.iter().any(|part| {
            let points = std::array::from_fn::<_, 9, _>(|i| {
                let local = if i == 8 { Vec3::ZERO } else {
                    Vec3::new(if i & 1 == 0 { -0.5 } else { 0.5 }, if i & 2 == 0 { -0.5 } else { 0.5 }, if i & 4 == 0 { -0.5 } else { 0.5 }) * part.size
                };
                at + turn * (part.at_rest(local) * scale)
            });
            self.points_seen_by_a_player(&points, angle)
        })
    }

    fn points_seen_by_a_player(&self, targets: &[Vec3], angle: f32) -> bool {
        if angle == 0.0 {
            return false;
        }
        let cos = angle.to_radians().cos();
        self.players.values().filter(|p| p.joined).any(|player| {
            let eye = player.eye();
            targets.iter().any(|&target| {
                let to = target - eye;
                let distance = to.length();
                distance < SEEN_WITHIN
                    && (distance < 0.001 || player.look().dot(to / distance) >= cos)
                    && voxmar_engine::physics::raycast(eye, to, distance, |cell| self.block(cell).is_some_and(|b| self.content.blocks.info(b).occludes)).is_none_or(|hit| hit.distance >= distance)
            })
        })
    }

    /// Room left under the world's total: what `most` leaves after the living and the kept
    /// animals still waiting in the save, so those always fit back.
    pub(crate) fn room_left(&self) -> usize {
        let animals = self.creatures.live.values().filter(|c| self.content.creatures.get(c.species).def.kind != Kind::Villager).count();
        self.content.creatures.limits().most.saturating_sub(animals + self.creatures.unplaced.len())
    }

    /// How many more wild creatures the world has room for: under the wild budget, which
    /// kept animals and a settlement's people never touch, and under the total.
    pub(crate) fn wild_room(&self) -> usize {
        let wild = self.creatures.live.values().filter(|c| c.owner.is_none() && self.content.creatures.get(c.species).def.kind != Kind::Villager).count();
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

    /// The cell nearest height `y` in a column where a body can stand: solid beneath (not a
    /// railing's narrow top), two free cells (or water) above.
    pub(crate) fn standing_spot(&self, x: i32, z: i32, y: i32) -> Option<IVec3> {
        let free = |cell: IVec3| self.block(cell).is_some_and(|b| !self.content.blocks.info(b).solid);
        // On footing: never on a railing's narrow top.
        let stands = |cell: IVec3| self.is_solid(cell - IVec3::Y) && !Ground::of(self).railing(cell - IVec3::Y) && free(cell) && free(cell + IVec3::Y);
        (0..24).flat_map(|d| [y + d, y - d - 1]).map(|cy| IVec3::new(x, cy, z)).find(|&cell| stands(cell))
    }

    /// Whether a block giving light, or a player holding a light (a firebrand), lies within
    /// `LIT_REACH` of `cell`.
    pub(crate) fn lit_near(&self, cell: IVec3) -> bool {
        let r = LIT_REACH;
        let held = self.players.values().filter(|p| p.joined && p.health.alive() && p.held().is_some_and(|item| self.content.tools.held_light(item) > 0));
        if held.into_iter().any(|p| (p.position.floor().as_ivec3() - cell).abs().max_element() <= r) {
            return true;
        }
        (-r..=r).any(|dy| (-r..=r).any(|dz| (-r..=r).any(|dx| self.block(cell + IVec3::new(dx, dy, dz)).is_some_and(|b| self.content.blocks.info(b).emission > 0))))
    }

    /// Wild creatures far from every player, or in columns that went, go.
    pub(crate) fn despawn_far(&mut self) {
        let players: Vec<Vec3> = self.players.values().filter(|p| p.joined).map(|p| p.position).collect();
        let gone: Vec<u32> = self
            .creatures
            .live
            .iter()
            .filter(|(id, c)| c.owner.is_none() && !c.companion && !self.villagers.manages(**id) && !self.wandering.manages(**id))
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

    /// Sends each player the creatures that changed within sight, and those gone from it:
    /// every other tick, but a new pose at once (a wind-up is a telegraph, and its blow lands
    /// a wind-up after it began, whenever the player is told).
    fn replicate_creatures(&mut self) {
        let gone = std::mem::take(&mut self.creatures.gone);
        let every_other = self.tick.is_multiple_of(2);
        let posed = |creature: &Live| creature.changed && creature.sent.is_some_and(|last| last.pose != creature.pose() || last.tell != creature.tell());
        for player in self.players.values().filter(|p| p.joined) {
            let seen = self.creatures.seen.entry(player.id).or_default();
            let mut changed = Vec::new();
            let mut left: Vec<CreatureGone> = gone.iter().filter(|g| seen.remove(&g.id)).copied().collect();
            for (&id, creature) in &self.creatures.live {
                let near = creature.body.position.distance(player.position) < SEEN_WITHIN;
                let known = seen.contains(&id);
                let moved = creature.changed && creature.sent.is_none_or(|last| creature.moved_since(&last));
                if near && (!known || (every_other && moved) || posed(creature)) {
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
        for creature in self.creatures.live.values_mut().filter(|c| c.changed) {
            if every_other {
                if creature.sent.is_none_or(|last| creature.moved_since(&last)) {
                    creature.sent = Some(creature.view(0));
                }
                creature.changed = false;
            } else if posed(creature) {
                // Sent: not again next tick unless it changes more.
                creature.sent = Some(creature.view(0));
            }
        }
    }
}

impl Ground<'_> {
    fn water_at(&self, cell: IVec3) -> bool {
        motion::Surroundings::water(self, cell)
    }
}

impl Server {
    /// Whether `cell` holds water a creature could drink or swim in.
    pub(crate) fn is_water(&self, cell: IVec3) -> bool {
        Ground::of(self).water_at(cell)
    }

    /// Whether any of `cells` holds something to drink (a block tagged `drinking_water`:
    /// water, a well, a trough), looked at in order.
    pub(crate) fn any_drink(&self, mut cells: impl Iterator<Item = IVec3>) -> bool {
        let (ground, blocks) = (Ground::of(self), &self.content.blocks);
        cells.any(|cell| ground.block(cell).is_some_and(|b| blocks.def(b).has_tag(voxmar_engine::creature::condition::DRINK)))
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
/// The budget near a player a species is counted against (`Limits`): villagers come with
/// their settlements and count against none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Budget {
    Animal,
    Bird,
    Monster,
}

pub(crate) fn budget(def: &creature::CreatureDef) -> Option<Budget> {
    match def.kind {
        Kind::Villager => None,
        Kind::Monster => Some(Budget::Monster),
        _ if def.movement.style == creature::Style::Flier => Some(Budget::Bird),
        _ => Some(Budget::Animal),
    }
}

fn spawn_factor(rarity: Rarity) -> f32 {
    match rarity {
        Rarity::Common => 1.0,
        Rarity::Uncommon => 0.3,
        Rarity::Rare => 0.08,
        Rarity::VeryRare => 0.015,
        Rarity::Legendary => 0.002,
    }
}

/// The first night can run past the nominal day boundary, but never into the second
/// night's darkness. Use the same dawn threshold as natural monster spawning.
fn first_night(day: f64, daylight: f32) -> bool {
    day < TICKS_PER_DAY as f64 || (day < 1.5 * TICKS_PER_DAY as f64 && daylight < 0.25)
}

/// The `i`th cell of the square ring `r` blocks out, as (dx, dz).
pub(crate) fn ring(r: i32, i: i32) -> (i32, i32) {
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

/// Whether a box goes `along` (horizontal) meeting nothing, swept in steps a tenth of a
/// block long: not an axis at a time, which goes round a corner (a stair's turn, a doorway's
/// frame) the straight line cuts through.
fn goes_straight(aabb: voxmar_engine::physics::Aabb, along: Vec3, solid: impl Fn(IVec3) -> bool) -> bool {
    let steps = (along.length() / 0.1).ceil().max(1.0) as i32;
    (1..=steps).all(|i| {
        let mut at = aabb;
        at.translate(Vec3::Y * 0.01 + along * (i as f32 / steps as f32));
        !at.intersects(&solid)
    })
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
mod neighbour_tests {
    use super::*;

    #[test]
    fn neighbours_find_what_looking_at_every_creature_finds() {
        let mut server = crate::test_server();
        // Three hundred of three kinds over 120 blocks square, a third of them ready to breed (two in three of those kept),
        // some on cell edges and some in negative cells.
        let kinds: Vec<CreatureId> = ["base:rabbit", "base:junglefowl", "base:mouflon"].map(|name| server.content.creatures.id(name).expect("species")).into();
        for i in 0..300u64 {
            let (x, z) = (unit_from(mix(i, 1)) * 120.0 - 60.0, unit_from(mix(i, 2)) * 120.0 - 60.0);
            let (x, z) = if i % 25 == 0 { ((x / CELL).round() * CELL, z) } else { (x, z) };
            let id = server.spawn_creature(kinds[i as usize % 3], Vec3::new(x, server.spawn.y, z), i).expect("room");
            if i % 3 == 0 {
                // Two in three kept (only kept animals are mates), the rest let go and wild.
                let creature = server.creatures.live.get_mut(&id).unwrap();
                creature.ready_until = 1;
                creature.owner = (i % 9 != 0).then(|| "test".to_string());
            }
        }
        let neighbours = Neighbours::of(&server.creatures.live);
        let ids: Vec<u32> = server.creatures.live.keys().copied().collect();
        let at = |id: &u32| server.creatures.live[id].body.position;
        assert!(ids.iter().any(|id| at(id).x < 0.0) && ids.iter().any(|id| at(id).x > 0.0), "all on one side of zero");
        assert!(ids.iter().filter(|id| at(id).x % CELL == 0.0).count() >= 10, "too few on cell edges");
        let (mut with_kin, mut with_mates) = (0, 0);
        for &id in &ids {
            let me = &server.creatures.live[&id];
            let everyone = |range: f32, ready: bool| -> Vec<Vec3> {
                server
                    .creatures
                    .live
                    .iter()
                    .filter(|&(&other, c)| other != id && c.species == me.species && (!ready || (c.ready_until > 0 && c.owner.is_some())) && c.body.position.distance(me.body.position) < range)
                    .map(|(_, c)| c.body.position)
                    .collect()
            };
            let kin = server.kin_of(id, &neighbours);
            assert_eq!(kin, everyone(KIN_RANGE, false), "kin of {id}");
            with_kin += usize::from(!kin.is_empty());
            let nearest = everyone(voxmar_engine::creature::husbandry::MATE_RANGE, true).into_iter().min_by(|a, b| a.distance(me.body.position).total_cmp(&b.distance(me.body.position)));
            let mate = server.mate_for(id, &neighbours);
            assert_eq!(mate, if me.ready_until > 0 { nearest } else { None }, "mate of {id}");
            with_mates += usize::from(mate.is_some());
        }
        assert!(with_kin > 100 && with_mates > 10, "too sparse to judge: {with_kin} with kin, {with_mates} with mates");
    }

    /// A full pen (the 64 kept animals a player may have, with a few of another kind among
    /// them) about 50 blocks across, round the corner where four cells meet and over the
    /// cells beyond, so some of a kind stand out of each other's reach and some cells' nine
    /// leave part of the pen out: each one's kin, mate and those
    /// its alarm reaches are what looking at every creature finds, asked twice over in one
    /// pass, with some having moved across a cell's edge since the pass began.
    #[test]
    fn a_full_pen_finds_what_looking_at_every_creature_finds() {
        let mut server = crate::test_server();
        let (sheep, hens) = (server.content.creatures.id("base:mouflon").expect("species"), server.content.creatures.id("base:junglefowl").expect("species"));
        let corner = Vec3::new((server.spawn.x / CELL).round() * CELL, server.spawn.y, (server.spawn.z / CELL).round() * CELL);
        for i in 0..70u64 {
            let at = corner + Vec3::new((i % 9) as f32 * 6.0 - 24.2, 0.0, (i / 9) as f32 * 6.0 - 24.2);
            let id = server.spawn_creature(if i % 10 == 3 { hens } else { sheep }, at, i).expect("room");
            let creature = server.creatures.live.get_mut(&id).unwrap();
            creature.owner = Some("test".to_string());
            creature.ready_until = u64::from(i % 4 == 0);
        }
        let neighbours = Neighbours::of(&server.creatures.live);
        let ids: Vec<u32> = server.creatures.live.keys().copied().collect();
        let (mut short, mut crossed) = (0, 0);
        for round in 0..2 {
            for &id in &ids {
                let me = &server.creatures.live[&id];
                let near = |range: f32, ready: bool| -> Vec<(u32, Vec3)> {
                    server
                        .creatures
                        .live
                        .iter()
                        .filter(|&(&other, c)| other != id && c.species == me.species && (!ready || (c.ready_until > 0 && c.owner.is_some())) && c.body.position.distance(me.body.position) < range)
                        .map(|(&other, c)| (other, c.body.position))
                        .collect()
                };
                let kin = near(KIN_RANGE, false);
                assert_eq!(server.kin_of(id, &neighbours), kin.iter().map(|&(_, at)| at).collect::<Vec<_>>(), "kin of {id} (round {round})");
                let kind = server.creatures.live.values().filter(|c| c.species == me.species).count();
                short += usize::from(kin.len() + 1 < kind);
                let nearest = near(voxmar_engine::creature::husbandry::MATE_RANGE, true).into_iter().map(|(_, at)| at).min_by(|a, b| a.distance(me.body.position).total_cmp(&b.distance(me.body.position)));
                assert_eq!(server.mate_for(id, &neighbours), if me.ready_until > 0 { nearest } else { None }, "mate of {id} (round {round})");
                // Its alarm frightens just its kin.
                if id % 7 == 0 {
                    for c in server.creatures.live.values_mut() {
                        c.mind.fear = 0.0;
                    }
                    server.carry_out(id, Decision { alarm: true, ..Default::default() }, &neighbours);
                    let alarmed: Vec<u32> = server.creatures.live.iter().filter(|(_, c)| c.mind.fear > 0.0).map(|(&other, _)| other).collect();
                    assert_eq!(alarmed, kin.iter().map(|&(other, _)| other).collect::<Vec<_>>(), "alarmed by {id} (round {round})");
                }
            }
            // Those beside a cell's edge step across it after the pass began, as bodies move
            // between thinks: kin are found where they are now.
            for &id in &ids {
                let body = &mut server.creatures.live.get_mut(&id).unwrap().body;
                if (body.position.x - corner.x).abs() < 0.5 {
                    let before = Neighbours::cell(body.position);
                    body.position.x += 0.9;
                    crossed += usize::from(Neighbours::cell(body.position) != before);
                }
            }
        }
        assert!(crossed > 0, "none crossed a cell's edge");
        assert!(short > 0, "every one of a kind was in reach of every other: the pen is too small to judge");
    }
}

#[cfg(test)]
mod combat_tests {
    use super::*;

    /// A wind-up shows the tick it begins, though creatures that only move are sent every
    /// other tick: its blow lands a wind-up after it began, told or not.
    #[test]
    fn a_wind_up_shows_the_tick_it_begins() {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        let player = 1;
        let mut joined = crate::player::Player::new(player, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        joined.joined = true;
        server.players.insert(player, joined);
        let species = server.content.creatures.id("base:brown_bear").expect("a bear");
        let bear = server.spawn_creature(species, server.spawn + Vec3::X * 1.5, 1).expect("room");
        // Facing away at first: the attacking update must carry the server's turn too.
        server.creatures.live.get_mut(&bear).unwrap().body.yaw = motion::yaw_of(Vec3::X);
        server.tick = 10;
        server.replicate_creatures();
        while let Ok(Some(_)) = client.try_recv() {}
        server.tick = 11;
        server.wind_up(bear, player);
        server.replicate_creatures();
        let shown = std::iter::from_fn(|| client.try_recv().ok().flatten()).find_map(|message| match message {
            ServerMessage::Creatures { changed, .. } => changed.into_iter().find(|v| v.id == bear && v.pose == brain::Pose::Attacking),
            _ => None,
        }).expect("the wind-up waited a tick to be shown");
        assert!((shown.yaw - motion::yaw_of(-Vec3::X)).abs() < 1e-4, "the first telegraph still faced away");
        // Sent once: the next tick sends it again only if it changed more.
        assert!(server.creatures.live[&bear].sent.is_some_and(|v| v.pose == brain::Pose::Attacking));
    }

    #[test]
    fn a_wind_up_tracks_before_its_deadline_and_keeps_its_aim_after() {
        let mut server = crate::test_server();
        let (_client, connection) = voxmar_protocol::local_pair();
        let player = 1;
        // Centre of the loaded column, above terrain: unloaded neighbours are walls.
        let origin = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin().as_vec3();
        let half = voxmar_engine::coords::CHUNK_SIZE as f32 * 0.5;
        let at = Vec3::new(origin.x + half, server.spawn.y + 32.0, origin.z + half);
        let joined = crate::player::Player::new(player, connection, at - Vec3::Z * 2.0, 2, server.tick, server.survival.max_health, &server.config.limits);
        server.players.insert(player, joined);
        let species = server.content.creatures.id("base:brown_bear").expect("a bear");
        let bear = server.spawn_creature(species, at, 1).expect("room");
        server.wind_up(bear, player);
        let until = server.creatures.live[&bear].tracking.expect("tracking").1;
        assert!(until > server.tick && until < server.creatures.live[&bear].held_until);
        // The target moves during tracking: the bear still turns toward them.
        server.players.get_mut(&player).unwrap().position = at + Vec3::X * 2.0;
        server.tick = until - 1;
        let before = server.creatures.live[&bear].body.yaw;
        server.move_creature(bear, 1.0 / TICKS_PER_SECOND as f32);
        let locked = server.creatures.live[&bear].body.yaw;
        assert!(locked < before - 0.01, "the bear did not track the moving target");
        // At the deadline, moving behind it no longer changes its aim.
        server.tick = until;
        server.players.get_mut(&player).unwrap().position = at - Vec3::X * 2.0;
        assert!(server.tracking_drive(bear).is_none());
        server.move_creature(bear, 1.0 / TICKS_PER_SECOND as f32);
        assert_eq!(server.creatures.live[&bear].body.yaw, locked, "the locked wind-up followed a late side-step");
    }

    #[test]
    fn defence_trains_only_from_a_creature_the_player_lately_hurt_and_a_match() {
        let mut server = crate::test_server();
        let (_client, connection) = voxmar_protocol::local_pair();
        let (player, other) = (1, 2);
        let mut joined = crate::player::Player::new(player, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
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
                server.players.get_mut(&player).unwrap().health = traits::Health::full(server.survival.max_health);
                let species = server.creatures.live[&attacker].species;
                server.creature_strikes(attacker, species, player, 1.0);
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
        // Real blows: one that misses starts no fight, one that lands does. (Each lands as a
        // plain blow with the bare hand, as `fight` lands a swing once it has wound up.)
        server.creatures.live.get_mut(&a).unwrap().fought = None;
        let fist = server.content.combat.wielded(None, |_| 1);
        let plain = voxmar_engine::combat::Stroke::of(voxmar_engine::combat::Blow::Plain, 1.0, false, false, false, false, &server.content.combat.feel);
        let (mut missed, mut landed) = (false, false);
        for _ in 0..200 {
            server.tick += 1;
            let health = server.creatures.live[&a].health;
            server.land_blow(player, a, &fist, plain);
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

    /// A kill's loot is rolled, and pops, by the world's counts of loot rolls and drops, not
    /// the tick it dies on, and each roll counts on.
    #[test]
    fn a_kills_loot_rolls_by_the_count_of_rolls_not_the_tick() {
        let killed = |tick: u64| {
            let mut server = crate::test_server();
            server.meta.draw_key = 0x0d1a;
            let wolf = server.content.creatures.id("base:wolf").unwrap();
            server.tick = tick;
            let mut found = Vec::new();
            for i in 0..12 {
                let id = server.spawn_creature(wolf, server.spawn + Vec3::X, i).expect("room");
                server.creature_dies(id);
                found.extend(server.creatures.falling_loot.drain(..).map(|(_, stack, body)| (stack.item, stack.count, body.position.to_array(), body.velocity.to_array())));
            }
            (found, server.meta.loot)
        };
        let (first, rolls) = killed(1_000);
        assert!(rolls > 0 && !first.is_empty(), "twelve wolves rolled {rolls} times for {first:?}");
        assert_eq!(killed(1_377), (first, rolls), "the same kills later dropped otherwise");
    }

    /// A blow, either way, is drawn by its count: struck at another tick, the same count
    /// does the same, and each blow counts on.
    #[test]
    fn a_blow_draws_by_its_count_not_its_tick() {
        let mut server = crate::test_server();
        let (_client, connection) = voxmar_protocol::local_pair();
        let player = 1;
        let mut joined = crate::player::Player::new(player, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        joined.joined = true;
        joined.draw_key = crate::player::player_draw_key(0x0d1a, "ada");
        server.players.insert(player, joined);
        let wolf = server.content.creatures.id("base:wolf").unwrap();
        let a = server.spawn_creature(wolf, server.spawn + Vec3::X, 1).expect("room");
        let fist = server.content.combat.wielded(None, |_| 1);
        let plain = voxmar_engine::combat::Stroke::of(voxmar_engine::combat::Blow::Plain, 1.0, false, false, false, false, &server.content.combat.feel);
        // Ten blows each way from count 0, at a tick: what each did.
        let blows = |server: &mut Server, tick: u64| {
            server.tick = tick;
            server.players.get_mut(&player).unwrap().draws = Default::default();
            (0..10)
                .map(|_| {
                    let full = server.content.creatures.get(wolf).def.health;
                    server.creatures.live.get_mut(&a).unwrap().health = full;
                    server.players.get_mut(&player).unwrap().health = traits::Health::full(server.survival.max_health);
                    let struck = server.land_blow(player, a, &fist, plain);
                    let taken = server.creature_strikes(a, wolf, player, 1.0);
                    (struck, taken)
                })
                .collect::<Vec<_>>()
        };
        let first = blows(&mut server, 10_000);
        assert_eq!(blows(&mut server, 10_377), first, "the same counts struck later drew otherwise");
        let draws = server.players[&player].draws;
        assert_eq!((draws.strikes, draws.blows), (10, 10), "each blow counted on");
        assert!(first.windows(2).any(|w| w[0] != w[1]), "ten blows each way all drew alike: {first:?}");
    }

    #[test]
    fn numbered_blows_land_as_often_as_the_odds_say() {
        // A player's blows at a level-1 creature, and a creature's at them, numbered by
        // their counts, for players of twenty draw keys.
        let chance = voxmar_engine::combat::hit_chance(1, 0, 1, 0);
        for kind in [Draw::Strike, Draw::Blow] {
            for player in 0..20u64 {
                let key = crate::player::player_draw_key(0x0d1a, &format!("player{player}"));
                let (mut landed, mut misses, mut longest) = (0, 0, 0);
                for blow in 0..400u64 {
                    let [hit, _] = crate::save::numbered_draws(key, kind.salt(), blow);
                    if hit < chance {
                        landed += 1;
                        misses = 0;
                    } else {
                        misses += 1;
                        longest = longest.max(misses);
                    }
                }
                let rate = landed as f32 / 400.0;
                assert!((rate - chance).abs() < 0.08, "{kind:?}, player {player}: {rate} landed, the odds are {chance}");
                assert!(longest < 20, "{kind:?}, player {player}: {longest} misses in a row");
            }
        }
    }
}

/// How many wild creatures a player meets: `density` fills a world around a player, by day
/// and by night, and counts them. `wild_density_probe` prints the table the numbers in
/// `limits` were chosen from (`cargo test -p voxmar_server wild_density_probe -- --ignored --nocapture`).
#[cfg(test)]
mod density_tests {
    use super::*;

    /// A dry, level pad in otherwise empty columns: these admission tests must not
    /// depend on the origin's generated seabed, water or vegetation.
    fn dry_spawn_pad(server: &mut Server, min: IVec3, max: IVec3) {
        let (from, to) = (ColumnPos::of_block(min), ColumnPos::of_block(max));
        for z in from.0.y..=to.0.y {
            for x in from.0.x..=to.0.x {
                server.world.insert(ColumnPos::new(x, z), std::sync::Arc::new(voxmar_engine::world::Column::default()));
            }
        }
        let stone = server.content.blocks.expect("base:stone");
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                let cell = IVec3::new(x, min.y, z);
                server.world.set_block(cell - IVec3::Y, stone);
                assert_eq!(server.standing_spot(x, z, min.y), Some(cell), "the fixture must admit the intended floor");
                assert!(!Ground::of(server).water_at(cell), "the fixture must offer dry land");
            }
        }
    }

    #[test]
    fn residents_leave_the_nearby_animal_budget_available() {
        let mut server = crate::test_server();
        let (_client, connection) = voxmar_protocol::local_pair();
        let at = server.spawn;
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        let cap = server.content.creatures.limits().animals_near;
        let farmer = server.content.creatures.id("base:farmer").unwrap();
        let sheep = server.content.creatures.id("base:mouflon").unwrap();
        for seed in 0..cap + 1 {
            server.spawn_seeded(farmer, at, seed as u64).expect("room for residents");
        }
        assert!(server.nearby_room(at, Budget::Animal, cap), "residents must not consume animal room");
        for salt in 0..cap {
            assert!(server.nearby_room(at, Budget::Animal, cap), "animal room vanished before the cap");
            server.spawn_creature(sheep, at, salt as u64).expect("room for animals");
        }
        assert!(!server.nearby_room(at, Budget::Animal, cap), "true animals must still exhaust the nearby budget");
        assert!(server.nearby_room(at, Budget::Monster, 1), "neither animals nor residents exhaust monster room");
        // Birds keep a budget of their own: a full field still has room for a few birds,
        // and birds never take the animals' places.
        let birds = server.content.creatures.limits().birds_near;
        let finch = server.content.creatures.id("base:goldfinch").unwrap();
        for salt in 0..birds {
            assert!(server.nearby_room(at, Budget::Bird, birds), "the animals took the birds' room");
            server.spawn_creature(finch, at, 100 + salt as u64).expect("room for birds");
        }
        assert!(!server.nearby_room(at, Budget::Bird, birds), "birds must exhaust their own budget");
        assert_eq!(budget(&server.content.creatures.get(finch).def), Some(Budget::Bird));
    }

    #[test]
    fn first_night_ends_at_dawn_even_when_darkness_crosses_the_day_boundary() {
        let day = TICKS_PER_DAY as f64;
        assert!(first_night(day - 1.0, 0.0));
        assert!(first_night(day, 0.0), "the nominal day end is not necessarily dawn");
        assert!(first_night(day + 1.0, 0.249));
        assert!(!first_night(day + 1.0, 0.25), "dawn releases the lower cap");
        assert!(!first_night(day * 1.5, 0.0), "later darkness is never night one");
        assert!(!first_night(day * 2.0, 0.0));
    }

    #[test]
    fn torch_boundaries_protect_every_member_of_a_monster_group() {
        let mut server = crate::test_server();
        let species = server.content.creatures.find("base:shambler").unwrap();
        let mut def = server.content.creatures.get(species).def.clone();
        (def.supernatural, def.habitat, def.group.size) = (false, Default::default(), (6, 6));
        let rules = creature::Rules { limits: creature::Limits { spawn_unseen_angle: 0.0, empty_for_monsters: 0.0, ..Default::default() }, ..Default::default() };
        server.content.creatures = std::sync::Arc::new(creature::CreatureRegistry::new(vec![def], rules, &server.content.blocks, &server.content.skills).unwrap());
        let y = 200;
        dry_spawn_pad(&mut server, IVec3::new(-44, y, -44), IVec3::new(44, y, 44));
        let torch = server.content.blocks.id("base:torch").unwrap();
        // Eleven lit cells out of twelve leave narrow dark lanes. A leader in a lane
        // may be allowed while its group offsets enter the adjacent torchlight.
        for z in -3..=3 {
            for x in -3..=3 {
                server.world.set_block(IVec3::new(x * 12, y, z * 12), torch);
            }
        }
        let (client, connection) = voxmar_protocol::local_pair();
        let _keep = client;
        let mut player = crate::player::Player::new(1, connection, Vec3::new(0.5, y as f32, 0.5), 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        server.day = (LATER + 20_000) as f64;
        assert!(server.daylight() < 0.25 && !first_night(server.day, server.daylight()));
        let (mut monsters, mut groups) = (0, 0);
        for salt in 0..400 {
            server.creatures.live.clear();
            server.tick = 40 + salt * 7;
            server.spawn_near(1, salt);
            for creature in server.creatures.live.values() {
                monsters += 1;
                assert!(!server.lit_near(creature.body.position.floor().as_ivec3()), "a group member entered torchlight");
            }
            groups += usize::from(server.creatures.live.len() > 1);
            assert!(server.creatures.live.len() <= rules.limits.monsters_near);
        }
        assert!(groups > 5, "only {groups} groups crossed the dark lanes");
        assert!(monsters > 20, "only {monsters} monsters in dark lanes; the fixture must exercise actual groups");
    }

    #[test]
    fn rendered_bat_wings_cannot_appear_around_a_wall() {
        let mut server = crate::test_server();
        let y = server.overworld.site(0, 0).height + 20;
        server.load_area(IVec3::new(-16, y, -16), IVec3::new(16, y, 16));
        let (client, connection) = voxmar_protocol::local_pair();
        let _keep = client;
        let mut player = crate::player::Player::new(1, connection, Vec3::new(0.5, y as f32, 0.5), 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        let stone = server.content.blocks.id("base:stone").unwrap();
        for h in y..=y + 3 {
            server.world.set_block(IVec3::new(0, h, -6), stone);
        }
        let bat = server.content.creatures.find("base:cave_bat").unwrap();
        let at = Vec3::new(1.3, y as f32 + 1.0, -11.5);
        let size = server.content.creatures.get(bat).def.body.size;
        let collision = std::array::from_fn::<_, 8, _>(|i| {
            at + Vec3::new(if i & 1 == 0 { -size.0 * 0.5 } else { size.0 * 0.5 }, if i & 2 == 0 { 0.0 } else { size.1 }, if i & 4 == 0 { -size.0 * 0.5 } else { size.0 * 0.5 })
        });
        assert!(!server.points_seen_by_a_player(&collision, 75.0), "the wall must hide every collision corner");
        assert!(server.model_seen_by_a_player(bat, at, 0.0, 1.0, 75.0), "a wing extends visibly past the wall");
        assert!(!server.model_seen_by_a_player(bat, at, std::f32::consts::FRAC_PI_2, 1.0, 75.0), "turning the bat folds its span behind the wall");
        assert!(!server.model_seen_by_a_player(bat, at, 0.0, 0.5, 75.0), "individual scale also changes the visible span");
        let mut rejected = 0;
        for salt in 0..200 {
            let seed = mix(mix(server.meta.seed, server.creatures.next as u64), salt);
            let yaw = unit_from(mix(seed, 3)) * std::f32::consts::TAU;
            let scale = 0.88 + 0.24 * unit_from(mix(seed, 0x5ca1e));
            if server.model_seen_by_a_player(bat, at, yaw, scale, 75.0) {
                assert!(server.spawn_unseen(bat, at, salt, 75.0).is_none(), "a naturally spawned wing appeared in view");
                rejected += 1;
            }
        }
        assert!(rejected > 20, "only {rejected} exposed bat poses exercised natural rejection");
    }

    #[test]
    fn natural_groups_respect_both_players_nearby_caps() {
        let mut server = crate::test_server();
        let species = server.content.creatures.find("base:cave_bat").unwrap();
        let mut def = server.content.creatures.get(species).def.clone();
        (def.habitat, def.group.size) = (Default::default(), (6, 6));
        let rules = creature::Rules { limits: creature::Limits { spawn_unseen_angle: 0.0, empty_for_monsters: 0.0, ..Default::default() }, ..Default::default() };
        server.content.creatures = std::sync::Arc::new(creature::CreatureRegistry::new(vec![def], rules, &server.content.blocks, &server.content.skills).unwrap());
        let species = CreatureId(0);
        let y = 200;
        dry_spawn_pad(&mut server, IVec3::new(-4, y, -44), IVec3::new(124, y, 44));
        let (a, ca) = voxmar_protocol::local_pair();
        let (b, cb) = voxmar_protocol::local_pair();
        let _keep = (a, b);
        for (id, x, connection) in [(1, 0.5, ca), (2, 80.5, cb)] {
            let mut player = crate::player::Player::new(id, connection, Vec3::new(x, y as f32, 0.5), 2, server.tick, server.survival.max_health, &server.config.limits);
            player.joined = true;
            server.players.insert(id, player);
        }
        server.day = 20_000.0;
        assert!(first_night(server.day, server.daylight()));
        let mut allowed = 0;
        for salt in 0..300 {
            server.creatures.live.clear();
            for i in 0..rules.limits.first_night_monsters {
                server.spawn_creature(species, Vec3::new(i as f32, y as f32, 0.0), i as u64).unwrap();
            }
            server.tick = 40 + salt * 7;
            server.spawn_near(2, salt);
            allowed += server.creatures.live.len() - rules.limits.first_night_monsters;
            for player in server.players.values() {
                let count = server.creatures.live.values().filter(|c| c.body.position.distance(player.position) < NEAR).count();
                assert!(count <= rules.limits.first_night_monsters, "player {} had {count} first-night monsters", player.id);
            }
        }
        assert!(allowed > 20, "only {allowed} allowed spawns outside the capped overlap");
    }

    /// The nearest place to the origin, spiralling out, whose biome is `biome`.
    pub(super) fn find_biome(server: &Server, biome: &str) -> Option<(i32, i32)> {
        let biomes = server.overworld.biomes();
        (0..60).find_map(|r| {
            (0..(8 * r).max(1)).find_map(|i| {
                let (dx, dz) = ring(r, i);
                let (x, z) = (dx * 100, dz * 100);
                (biomes.get(server.overworld.site(x, z).biome).name == format!("base:{biome}")).then_some((x, z))
            })
        })
    }

    /// Past the world's first day, whose night brings fewer monsters.
    pub(super) const LATER: u64 = 3 * TICKS_PER_DAY;

    /// Counts after `minutes` of server time with a player standing at `spot` at `time` of
    /// day (ticks since the world began; `LATER` is days past its first, whose night is
    /// quieter), lit by torches or not: wild animals within 32, 64 and 128 blocks, then monsters
    /// within 64 and 128.
    pub(super) fn density(server: &mut Server, spot: (i32, i32), time: u64, minutes: u64, torches: bool) -> [usize; 5] {
        let (client, connection) = voxmar_protocol::local_pair();
        let _keep = client;
        let height = server.overworld.site(spot.0, spot.1).height;
        let at = Vec3::new(spot.0 as f32 + 0.5, height as f32 + 1.0, spot.1 as f32 + 0.5);
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        (player.joined, player.admin, player.name) = (true, true, "probe".into());
        server.players.clear();
        server.players.insert(1, player);
        server.creatures.live.clear();
        let cell = at.floor().as_ivec3();
        server.load_area(cell - IVec3::new(150, 0, 150), cell + IVec3::new(150, 0, 150));
        if torches {
            // A torch every 4 blocks over 150 blocks square, on the ground at three heights
            // (hills, ledges): no spot is without light.
            let torch = server.content.blocks.id("base:torch").expect("torches");
            let mut cells: Vec<IVec3> = (-18..=18)
                .flat_map(|i| (-18..=18).map(move |j| (i, j)))
                .flat_map(|(i, j)| [-20, 0, 20].map(|dy| server.standing_spot(cell.x + i * 4, cell.z + j * 4, cell.y + dy)))
                .flatten()
                .collect();
            cells.sort_by_key(|c| (c.x, c.z, c.y));
            cells.dedup();
            let changes: Vec<(IVec3, BlockId)> = cells.into_iter().map(|c| (c, torch)).collect();
            server.set_blocks(&changes);
        }
        server.day = time as f64;
        for _ in 0..minutes * 60 * TICKS_PER_SECOND as u64 {
            server.tick += 1;
            server.tick_creatures();
        }
        let mut counts = [0; 5];
        for creature in server.creatures.live.values().filter(|c| c.owner.is_none()) {
            let monster = server.content.creatures.get(creature.species).def.kind == Kind::Monster;
            let distance = creature.body.position.distance(at);
            let slots: &[(usize, f32)] = if monster { &[(3, 64.0), (4, 128.0)] } else { &[(0, 32.0), (1, 64.0), (2, 128.0)] };
            slots.iter().filter(|(_, within)| distance < *within).for_each(|(i, _)| counts[*i] += 1);
        }
        counts
    }

    /// The world is not empty around a player (Mariano, 2026-10-08: "the world is feeling a
    /// bit dead"): in a few minutes, standing still in the open by day there are a dozen
    /// animals within 64 blocks and some within sight, and by night monsters too. With the
    /// halved caps of 6 (and a draw that left a spot empty twice in three) it was 6 and 1.
    #[test]
    fn a_player_in_the_open_has_company_by_day_and_a_threat_by_night() {
        let mut server = crate::test_server();
        let spot = find_biome(&server, "plains").expect("plains");
        let noon = (0..24_000u64).step_by(100).max_by(|a, b| creature::daylight(*a).total_cmp(&creature::daylight(*b))).unwrap();
        let midnight = (0..24_000u64).step_by(100).min_by(|a, b| creature::daylight(*a).total_cmp(&creature::daylight(*b))).unwrap();
        let [near, animals, ..] = density(&mut server, spot, noon + LATER, 3, false);
        assert!(animals >= 10 && near >= 3, "{animals} animals within 64 blocks, {near} within 32, after three minutes by day");
        let [_, _, _, monsters, _] = density(&mut server, spot, midnight + LATER, 3, false);
        assert!(monsters >= 7, "{monsters} monsters within 64 blocks after three minutes of night");
    }

    /// The first night of a new world brings 3 at most (docs/creatures.md); torches keep
    /// monsters from spawning where they light, on the surface too. One server stands for
    /// all three (`density` clears its players and creatures, and the first night is told
    /// by the time of day alone), so the land around the spot is made once; the torches
    /// come last.
    #[test]
    fn the_first_night_is_light_and_torches_keep_monsters_away() {
        let midnight = (0..24_000u64).step_by(100).min_by(|a, b| creature::daylight(*a).total_cmp(&creature::daylight(*b))).unwrap();
        let mut server = crate::test_server();
        let spot = find_biome(&server, "plains").expect("plains");
        let [_, _, _, first, _] = density(&mut server, spot, midnight, 3, false);
        assert!((1..=3).contains(&first), "{first} monsters on the first night");
        // Darkness that runs on past the first 24000 ticks is still the first night.
        if let Some(dawn) = (TICKS_PER_DAY..TICKS_PER_DAY * 3 / 2).find(|&t| creature::daylight(t) >= 0.25).filter(|&t| t > TICKS_PER_DAY) {
            let [_, _, _, late, _] = density(&mut server, spot, dawn - 1, 3, false);
            assert!(late <= 3, "{late} monsters in the first night's last dark before dawn");
        }
        let [_, _, _, lit, _] = density(&mut server, spot, midnight + LATER, 3, true);
        assert_eq!(lit, 0, "{lit} monsters among torches");
    }

    /// Nothing spawns where the player looks and could see it: of 400 tries around a player
    /// looking one way on open ground, every creature that appears lies out of their view,
    /// and some do appear (behind them, to the sides).
    #[test]
    fn nothing_spawns_in_the_players_view() {
        let mut server = crate::test_server();
        let spot = find_biome(&server, "plains").expect("plains");
        let (client, connection) = voxmar_protocol::local_pair();
        let _keep = client;
        let height = server.overworld.site(spot.0, spot.1).height;
        let at = Vec3::new(spot.0 as f32 + 0.5, height as f32 + 1.0, spot.1 as f32 + 0.5);
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        (player.joined, player.admin, player.name, player.yaw, player.pitch) = (true, true, "probe".into(), 0.0, 0.0);
        server.players.insert(1, player);
        let cell = at.floor().as_ivec3();
        server.load_area(cell - IVec3::new(80, 0, 80), cell + IVec3::new(80, 0, 80));
        server.day = (LATER + 6_000) as f64;
        let angle = server.content.creatures.limits().spawn_unseen_angle;
        let mut draws = Vec::new();
        for _ in 0..2 {
            (server.creatures.next, server.meta.rolls) = (1, 0);
            let (mut seen, mut behind) = (0, 0);
            let mut draw = Vec::new();
            for salt in 0..400u64 {
                server.creatures.live.clear();
                server.tick = 40 + salt * 7;
                server.spawn_near(1, salt);
                for (&id, creature) in &server.creatures.live {
                    let cell = creature.body.position.floor().as_ivec3();
                    seen += usize::from(server.seen_by_a_player(cell, angle) || server.model_seen_by_a_player(creature.species, creature.body.position, creature.body.yaw, creature.scale, angle));
                    behind += usize::from(creature.body.position.z > at.z);
                    draw.push((salt, id, creature.species, creature.seed, creature.body.position.to_array()));
                }
                assert!(server.creatures.live.len() <= server.content.creatures.limits().animals_near);
            }
            assert_eq!(seen, 0, "{seen} creatures appeared in view");
            assert!(behind >= 20, "only {behind} appeared behind the player");
            draw.sort_by_key(|(salt, id, ..)| (*salt, *id));
            draws.push(draw);
        }
        assert_eq!(draws[0], draws[1], "the same loaded world, count of rolls and players must draw the same creatures");
    }

    /// Where a creature comes near a player, and what it is like, is drawn by the world's
    /// count of rolls, never the tick it is tried on.
    #[test]
    fn a_spawn_draws_by_the_worlds_count_not_the_tick() {
        let content = crate::test_content();
        let spawned = |tick: u64| {
            let mut server = crate::test_server_on(content.clone());
            server.meta.draw_key = 0x0d1a;
            let spot = find_biome(&server, "plains").expect("plains");
            let height = server.overworld.site(spot.0, spot.1).height;
            let at = Vec3::new(spot.0 as f32 + 0.5, height as f32 + 1.0, spot.1 as f32 + 0.5);
            let (client, connection) = voxmar_protocol::local_pair();
            std::mem::forget(client);
            let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
            player.joined = true;
            server.players.insert(1, player);
            let cell = at.floor().as_ivec3();
            server.load_area(cell - IVec3::new(80, 0, 80), cell + IVec3::new(80, 0, 80));
            server.day = (LATER + 6_000) as f64;
            server.tick = tick;
            let mut made = Vec::new();
            for salt in 0..60u64 {
                server.creatures.live.clear();
                server.spawn_near(1, salt);
                made.extend(server.creatures.live.values().map(|c| (salt, c.species, c.seed, c.body.position.to_array())));
                server.tick += 7;
            }
            (made, server.meta.rolls)
        };
        let first = spawned(40);
        assert!(!first.0.is_empty() && first.1 >= 60, "{} spawned over {} rolls", first.0.len(), first.1);
        assert_eq!(spawned(40_977), first, "the same tries later drew otherwise");
    }

    #[test]
    fn unseen_spawning_checks_other_players_and_heads_above_walls() {
        let mut server = crate::test_server();
        let y = server.overworld.site(0, 0).height + 20;
        server.load_area(IVec3::new(-16, y, -16), IVec3::new(16, y, 16));
        let (client, connection) = voxmar_protocol::local_pair();
        let _keep = client;
        let mut player = crate::player::Player::new(1, connection, Vec3::new(0.5, y as f32, 0.5), 2, server.tick, server.survival.max_health, &server.config.limits);
        (player.joined, player.yaw, player.pitch) = (true, 0.0, 0.0);
        server.players.insert(1, player);
        let front = IVec3::new(0, y, -12);
        let back = IVec3::new(0, y, 12);
        assert!(server.seen_by_a_player(front, 75.0));
        assert!(!server.seen_by_a_player(back, 75.0));
        assert!(server.seen_by_a_player(back, 180.0));
        assert!(!server.seen_by_a_player(front, 0.0), "zero disables the rule");
        let shambler = server.content.creatures.find("base:shambler").unwrap();
        let next = server.creatures.next;
        assert!(server.spawn_unseen(shambler, front.as_vec3() + Vec3::splat(0.5), 7, 75.0).is_none());
        assert_eq!(server.creatures.next, next, "an unseen rejection consumes no creature id");
        assert!(server.creatures.live.is_empty());
        assert!(server.spawn_creature(shambler, front.as_vec3(), 7).is_some(), "explicit summons still work in view");

        let stone = server.content.blocks.id("base:stone").unwrap();
        let low_wall: Vec<_> = (-3..=3).flat_map(|x| (y..=y + 1).map(move |h| (IVec3::new(x, h, -6), stone))).collect();
        server.set_blocks(&low_wall);
        assert!(!server.seen_by_a_player(front, 75.0), "the wall hides the fixed middle point");
        assert!(server.model_seen_by_a_player(shambler, front.as_vec3() + Vec3::new(0.5, 0.0, 0.5), 0.0, 2.0, 75.0), "the head above the wall is still visible");
        let high_wall: Vec<_> = (-3..=3).flat_map(|x| (y + 2..=y + 5).map(move |h| (IVec3::new(x, h, -6), stone))).collect();
        server.set_blocks(&high_wall);
        assert!(!server.model_seen_by_a_player(shambler, front.as_vec3() + Vec3::new(0.5, 0.0, 0.5), 0.0, 2.0, 75.0), "a fully hidden body may spawn");

        let glass = server.content.blocks.id("base:glass").unwrap();
        assert!(server.content.blocks.info(glass).solid && !server.content.blocks.info(glass).occludes);
        let glass_wall: Vec<_> = (-3..=3).flat_map(|x| (y..=y + 5).map(move |h| (IVec3::new(x, h, -6), glass))).collect();
        server.set_blocks(&glass_wall);
        assert!(server.seen_by_a_player(front, 75.0), "solid glass is transparent to a player's view");
        assert!(server.model_seen_by_a_player(shambler, front.as_vec3() + Vec3::new(0.5, 0.0, 0.5), 0.0, 2.0, 75.0));

        let (client, connection) = voxmar_protocol::local_pair();
        let _keep_other = client;
        let mut other = crate::player::Player::new(2, connection, Vec3::new(0.5, y as f32, 0.5), 2, server.tick, server.survival.max_health, &server.config.limits);
        (other.joined, other.yaw, other.pitch) = (true, std::f32::consts::PI, 0.0);
        server.players.insert(2, other);
        assert!(server.seen_by_a_player(back, 75.0), "the second player can see behind the first");
        server.players.get_mut(&2).unwrap().joined = false;
        assert!(!server.seen_by_a_player(back, 75.0), "a player who has not joined has no view");
    }

    /// How long a kill of something with meat on it takes in the wild, for the pacing
    /// audits (design/farm-pacing.md): after the world fills round a player by day, the
    /// player sprints straight at the nearest wild animal that drops raw meat, over level
    /// ground (terrain and the animal's flight once struck are left out, so this is a
    /// floor), swings a stone axe at it, and goes on to the next, giving up on one it has
    /// chased for 20 s, while the world spawns and moves its animals as it would.
    /// `cargo test -p voxmar_server hunt_pace_probe -- --ignored --nocapture`.
    #[test]
    #[ignore = "prints a table; minutes of server time per row"]
    fn hunt_pace_probe() {
        const SPRINT: f32 = 6.1;
        const KILLS: u32 = 12;
        // A swing a beat (0.7 s), and about one and a half health a swing with a stone axe.
        const BEAT: u64 = 14;
        const HIT: f32 = 1.5;
        let mut server = crate::test_server();
        let noon = (0..24_000u64).step_by(100).max_by(|a, b| creature::daylight(*a).total_cmp(&creature::daylight(*b))).unwrap();
        println!("{:>10} | kills  seconds  s/kill  sprinting s/kill", "biome");
        for biome in ["plains", "forest", "meadow", "savanna"] {
            let Some(spot) = find_biome(&server, biome) else {
                println!("{biome:>10}: none found");
                continue;
            };
            density(&mut server, spot, noon + LATER, 3, false);
            let meaty: Vec<bool> = (0..server.content.creatures.len())
                .map(|i| {
                    let def = server.content.creatures.get(CreatureId(i as _));
                    def.def.kind == Kind::Animal && def.loot.iter().any(|l| server.content.blocks.def(l.item).name.contains(":raw_"))
                })
                .collect();
            let start = server.tick;
            let (mut kills, mut running, mut marks) = (0u32, 0u64, Vec::new());
            // What it gave up on (out-ran it for 20 s, as a player would), and what it is after.
            let (mut given_up, mut chase) = (FxHashSet::default(), (u32::MAX, 0u64));
            while kills < KILLS && server.tick - start < 20 * 60 * TICKS_PER_SECOND as u64 {
                let at = server.players[&1].position;
                let prey = server
                    .creatures
                    .live
                    .iter()
                    .filter(|(id, c)| c.owner.is_none() && meaty[c.species.0 as usize] && !given_up.contains(*id))
                    .map(|(&id, c)| (id, c.body.position, c.species))
                    .min_by(|a, b| a.1.distance(at).total_cmp(&b.1.distance(at)));
                let Some((id, p, species)) = prey else {
                    for _ in 0..TICKS_PER_SECOND {
                        server.tick += 1;
                        server.tick_creatures();
                    }
                    continue;
                };
                let name = server.content.creatures.get(species).def.name.trim_start_matches("base:").to_owned();
                if chase.0 != id {
                    chase = (id, server.tick);
                } else if server.tick - chase.1 > 20 * TICKS_PER_SECOND as u64 {
                    given_up.insert(id);
                    marks.push(format!("(gave up on a {name})"));
                    continue;
                }
                let gap = Vec2::new(p.x - at.x, p.z - at.z);
                if gap.length() <= 2.0 {
                    let health = server.content.creatures.get(species).def.health as f32;
                    for _ in 0..(health / HIT).ceil() as u64 * BEAT {
                        server.tick += 1;
                        server.tick_creatures();
                    }
                    server.creatures.live.remove(&id);
                    kills += 1;
                    marks.push(format!("{name}@{:.0}", (server.tick - start) as f32 / TICKS_PER_SECOND as f32));
                    continue;
                }
                let step = gap.normalize() * (SPRINT / TICKS_PER_SECOND as f32).min(gap.length());
                server.players.get_mut(&1).unwrap().position = Vec3::new(at.x + step.x, p.y, at.z + step.y);
                running += 1;
                server.tick += 1;
                server.tick_creatures();
            }
            let seconds = (server.tick - start) as f32 / TICKS_PER_SECOND as f32;
            let sprint = running as f32 / TICKS_PER_SECOND as f32;
            println!("{biome:>10} | {kills:>5} {seconds:>8.1} {:>7.1} {:>16.1}", seconds / kills.max(1) as f32, sprint / kills.max(1) as f32);
            println!("{:>10}   {}", "", marks.join(" "));
        }
    }

    #[test]
    #[ignore = "prints a table; minutes of server time per row"]
    fn wild_density_probe() {
        let mut server = crate::test_server();
        // Noon, and the middle of the night (when the sun is lowest).
        let noon = (0..24_000u64).step_by(100).max_by(|a, b| creature::daylight(*a).total_cmp(&creature::daylight(*b))).unwrap();
        let midnight = (0..24_000u64).step_by(100).min_by(|a, b| creature::daylight(*a).total_cmp(&creature::daylight(*b))).unwrap();
        println!("{:>14} {:>6} | animals 32  64  128 | monsters 64  128", "biome", "time");
        for biome in ["plains", "forest", "taiga", "jungle", "savanna", "mediterranean", "swamp"] {
            let Some(spot) = find_biome(&server, biome) else {
                println!("{biome:>14}: none found");
                continue;
            };
            for (label, time) in [("day", noon), ("night", midnight)] {
                let [a32, a64, a128, m64, m128] = density(&mut server, spot, time + LATER, 5, false);
                println!("{biome:>14} {label:>6} | {a32:>10} {a64:>3} {a128:>4} | {m64:>10} {m128:>4}");
            }
        }
    }
}

/// Creatures hurt beside walls, in corners and by trees: they run clear, round what is in
/// their way, and their bodies (heads too) never go into a block.
#[cfg(test)]
pub(crate) mod cornered_tests {
    use super::*;

    const SPECIES: [&str; 4] = ["base:red_deer", "base:rabbit", "base:mouflon", "base:wild_boar"];

    /// A stone pen over empty columns, high over spawn: a floor `half` blocks each way from
    /// its centre, walled three high round its edge. Returns the centre, at feet level.
    pub(crate) fn pen(server: &mut Server, half: i32) -> IVec3 {
        let centre = server.spawn.floor().as_ivec3() + IVec3::Y * 40;
        let (from, to) = (ColumnPos::of_block(centre - IVec3::splat(half)), ColumnPos::of_block(centre + IVec3::splat(half)));
        for z in from.0.y..=to.0.y {
            for x in from.0.x..=to.0.x {
                server.world.insert(ColumnPos::new(x, z), std::sync::Arc::new(voxmar_engine::world::Column::default()));
            }
        }
        let stone = server.content.blocks.expect("base:stone");
        for z in -half..=half {
            for x in -half..=half {
                server.world.set_block(centre + IVec3::new(x, -1, z), stone);
            }
        }
        let rim = (-half..=half).flat_map(|i| [IVec3::new(i, 0, -half), IVec3::new(i, 0, half), IVec3::new(-half, 0, i), IVec3::new(half, 0, i)]);
        wall(server, rim.map(|cell| centre + cell).collect::<Vec<_>>(), 3);
        centre
    }

    /// Stone `high` blocks tall on each of `cells` (feet level).
    pub(super) fn wall(server: &mut Server, cells: impl IntoIterator<Item = IVec3>, high: i32) {
        let stone = server.content.blocks.expect("base:stone");
        for cell in cells {
            for y in 0..high {
                server.world.set_block(cell + IVec3::Y * y, stone);
            }
        }
    }

    pub(crate) fn join(server: &mut Server, at: Vec3) -> PlayerId {
        let (_client, connection) = voxmar_protocol::local_pair();
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        1
    }

    /// A creature of `name` at `at`, facing along `facing`.
    pub(crate) fn put(server: &mut Server, name: &str, at: Vec3, facing: Vec3) -> u32 {
        let species = server.content.creatures.id(name).expect("species");
        let id = server.spawn_creature(species, at, 7).expect("room");
        server.creatures.live.get_mut(&id).unwrap().body.yaw = motion::yaw_of(facing);
        id
    }

    /// Prey that gores back while it is hale (a wild boar) runs once badly hurt: put in at a
    /// tenth of its health, so struck it runs as the rest do.
    pub(super) fn badly_hurt(server: &mut Server, id: u32) {
        let live = server.creatures.live.get_mut(&id).unwrap();
        let species = server.content.creatures.get(live.species);
        if species.fights() && !species.def.boss {
            live.health = (species.def.health / 10).max(1);
        }
    }

    /// Runs `seconds` of creature ticks, the player landing a harmless blow on `id` each
    /// second it is within reach, and checks every tick that no body or head is in a
    /// block. Returns the furthest it got from the player, and whether it struck at them
    /// (began a blow; the player is healed each tick).
    fn hunted(server: &mut Server, id: u32, player: PlayerId, seconds: f32) -> (f32, bool) {
        let (mut furthest, mut struck) = (0.0f32, false);
        for t in 0..(seconds * TICKS_PER_SECOND as f32) as u64 {
            server.tick += 1;
            let reach = server.players[&player].position.distance(server.creatures.live[&id].body.position);
            if t % TICKS_PER_SECOND as u64 == 0 && reach < 3.5 {
                server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
            }
            server.think_and_move();
            let ground = Ground::of(server);
            for (&other, c) in &server.creatures.live {
                assert!(!c.body.aabb().intersects(|p| motion::Surroundings::solid(&ground, p)), "creature {other} went into a block at {} (tick {t})", c.body.position);
                assert!(!c.body.head_blocked(&ground), "creature {other}'s head went into a block at {} (tick {t})", c.body.position);
            }
            furthest = furthest.max(server.players[&player].position.distance(server.creatures.live[&id].body.position));
            struck |= server.creatures.live[&id].decision.attack.is_some();
            server.players.get_mut(&player).unwrap().health = traits::Health::full(server.survival.max_health);
        }
        (furthest, struck)
    }

    /// One of `name` put at `at` facing `facing`, if its kind fights back, struck at full
    /// health: it stands and fights, striking the player within 3 s, and (`hunted`) never
    /// puts its body or head in a block doing it.
    fn fights_back_hale(server: &mut Server, name: &str, at: Vec3, facing: Vec3, player: PlayerId) {
        let species = server.content.creatures.id(name).expect("species");
        if !server.content.creatures.get(species).fights() {
            return;
        }
        let id = put(server, name, at, facing);
        let (_, struck) = hunted(server, id, player, 3.0);
        assert!(struck, "{name} struck at full health did not strike back within 3 s");
        server.creatures.live.remove(&id);
    }

    /// Struck with its back to a wall, the way it would run is shut: it runs along the wall
    /// and gets clear, rather than pressing into it under the player's blows.
    #[test]
    fn a_creature_struck_against_a_wall_runs_clear_along_it() {
        let mut server = crate::test_server();
        let centre = pen(&mut server, 16);
        wall(&mut server, (-16..=16).map(|z| centre + IVec3::new(-3, 0, z)).collect::<Vec<_>>(), 3);
        let player = join(&mut server, centre.as_vec3() + Vec3::new(1.5, 0.0, 0.5));
        for name in SPECIES {
            fights_back_hale(&mut server, name, centre.as_vec3() + Vec3::new(-1.5, 0.0, 0.5), Vec3::X, player);
            let id = put(&mut server, name, centre.as_vec3() + Vec3::new(-1.5, 0.0, 0.5), Vec3::X);
            badly_hurt(&mut server, id);
            let (away, _) = hunted(&mut server, id, player, 5.0);
            assert!(away > 6.0, "{name} struck against a wall got only {away:.1} blocks from the player");
            server.creatures.live.remove(&id);
        }
    }

    /// A pen walled two high keeps its animals struck, running or fighting back (a kept
    /// boar gores its keeper), tall heads and all: a head over the wall is no step (motion's head-met step is judged at the feet).
    #[test]
    fn a_pen_two_high_keeps_its_animals_struck_whether_they_run_or_fight() {
        let mut server = crate::test_server();
        let half = 5;
        let centre = pen(&mut server, half);
        for i in -half..=half {
            for cell in [IVec3::new(i, 2, -half), IVec3::new(i, 2, half), IVec3::new(-half, 2, i), IVec3::new(half, 2, i)] {
                server.world.set_block(centre + cell, voxmar_engine::block::BlockId::AIR);
            }
        }
        let player = join(&mut server, centre.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
        for name in ["base:red_deer", "base:wild_boar", "base:aurochs", "base:brown_bear"] {
            let Some(species) = server.content.creatures.id(name) else { continue };
            let id = put(&mut server, name, centre.as_vec3() + Vec3::new(-2.5, 0.0, 0.5), Vec3::NEG_X);
            server.creatures.live.get_mut(&id).unwrap().owner = Some("1".into());
            let _ = hunted(&mut server, id, player, 10.0);
            let at = server.creatures.live[&id].body.position - centre.as_vec3();
            assert!(at.x.abs() < half as f32 && at.z.abs() < half as f32 && at.y < 1.5, "{name} ({species:?}) left a pen two high: at {at}");
            server.creatures.live.remove(&id);
        }
    }

    /// Struck in a corner, with the player in front: it finds the way out, past them (and,
    /// the pen being small, runs round it).
    #[test]
    fn a_creature_struck_in_a_corner_finds_the_way_out() {
        let mut server = crate::test_server();
        let centre = pen(&mut server, 6);
        let corner = centre.as_vec3() + Vec3::new(-4.4, 0.0, -4.4);
        let player = join(&mut server, corner + Vec3::new(2.5, 0.0, 2.5));
        for name in SPECIES {
            fights_back_hale(&mut server, name, corner, Vec3::new(1.0, 0.0, 1.0).normalize(), player);
            let id = put(&mut server, name, corner, Vec3::new(1.0, 0.0, 1.0).normalize());
            badly_hurt(&mut server, id);
            let (away, _) = hunted(&mut server, id, player, 6.0);
            assert!(away > 5.0, "{name} struck in a corner got only {away:.1} blocks from the player");
            server.creatures.live.remove(&id);
        }
    }

    /// A tree trunk right behind it: it runs round the trunk, not into it.
    #[test]
    fn a_creature_struck_before_a_tree_runs_round_it() {
        let mut server = crate::test_server();
        let centre = pen(&mut server, 16);
        let log = server.content.blocks.expect("base:oak_log");
        for y in 0..6 {
            server.world.set_block(centre + IVec3::new(-3, y, 0), log);
        }
        let player = join(&mut server, centre.as_vec3() + Vec3::new(1.5, 0.0, 0.5));
        for name in SPECIES {
            fights_back_hale(&mut server, name, centre.as_vec3() + Vec3::new(-1.3, 0.0, 0.5), Vec3::X, player);
            let id = put(&mut server, name, centre.as_vec3() + Vec3::new(-1.3, 0.0, 0.5), Vec3::X);
            badly_hurt(&mut server, id);
            let (away, _) = hunted(&mut server, id, player, 5.0);
            assert!(away > 6.0, "{name} struck before a tree got only {away:.1} blocks from the player");
            server.creatures.live.remove(&id);
        }
    }

    /// A block set right before its face (by a player building, say) does not lock it
    /// there: it turns away and walks off, where it used to stand frozen for good.
    #[test]
    fn a_creature_with_a_block_before_its_face_turns_away() {
        let mut server = crate::test_server();
        let centre = pen(&mut server, 16);
        let player = join(&mut server, centre.as_vec3() + Vec3::new(4.5, 0.0, 0.5));
        let id = put(&mut server, "base:red_deer", centre.as_vec3() + Vec3::new(0.5, 0.0, 0.5), -Vec3::X);
        // A tick standing gives its body its head's reach.
        server.move_creature(id, 0.0);
        let body = server.creatures.live[&id].body;
        let face = (body.position + body.forward() * body.reach + Vec3::Y * body.head).floor().as_ivec3();
        server.world.set_block(face, server.content.blocks.expect("base:stone"));
        assert!(body.head_blocked(&Ground::of(&server)), "the fixture must put the block at its head");
        let start = body.position;
        server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
        for _ in 0..3 * TICKS_PER_SECOND {
            server.tick += 1;
            server.think_and_move();
        }
        let now = server.creatures.live[&id].body;
        assert!(!now.head_blocked(&Ground::of(&server)), "its head stayed in the block");
        assert!(now.position.distance(start) > 3.0, "it stood frozen by the block at its face");
    }
}

/// Movement over the land a player meets in the first ages: hunters come at a player round
/// trees, up steps and through doorways and open doors (never a shut one); a sheep after wheat
/// follows through a doorway too; prey runs along a cliff edge or a lake shore rather than
/// off it; and none is ever made with its head in a trunk.
#[cfg(test)]
mod way_tests {
    use super::cornered_tests::{join, pen, put, wall};
    use super::*;

    /// Seconds for creature `id` to come within a couple of blocks of the player at `to`
    /// (and up to their level), if it does within `seconds`. The player is never hurt.
    fn comes(server: &mut Server, id: u32, to: Vec3, seconds: u64) -> Option<f32> {
        let player = 1;
        server.players.get_mut(&player).unwrap().position = to;
        for t in 0..seconds * TICKS_PER_SECOND as u64 {
            server.tick += 1;
            server.players.get_mut(&player).unwrap().health = traits::Health::full(server.survival.max_health);
            server.think_and_move();
            let at = server.creatures.live[&id].body.position;
            if at.with_y(0.0).distance(to.with_y(0.0)) < 2.4 && (at.y - to.y).abs() < 0.6 {
                return Some(t as f32 / TICKS_PER_SECOND as f32);
            }
        }
        None
    }

    /// A wall across the pen at x = 1, with a gap at z = 3 that `door` (a door's lower half,
    /// shut or open, its upper half named after it) fills, or nothing.
    fn walled(server: &mut Server, c: IVec3, door: Option<&str>) {
        wall(server, (-14..=14).filter(|&z| z != 3).map(|z| c + IVec3::new(1, 0, z)).collect::<Vec<_>>(), 3);
        if let Some(door) = door {
            let (lower, upper) = (server.content.blocks.expect(door), server.content.blocks.expect(&format!("{door}_top")));
            server.world.set_block(c + IVec3::new(1, 0, 3), lower);
            server.world.set_block(c + IVec3::new(1, 1, 3), upper);
        }
    }

    /// An angered `name` set against a player five blocks off over `scene`.
    fn hunter(name: &str, scene: impl Fn(&mut Server, IVec3) -> Vec3, seconds: u64) -> Option<f32> {
        let mut server = crate::test_server();
        let c = pen(&mut server, 14);
        let to = scene(&mut server, c);
        let player = join(&mut server, to);
        let from = c.as_vec3() + Vec3::new(-4.5, 0.0, 0.5);
        let id = put(&mut server, name, from, to - from);
        server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
        comes(&mut server, id, to, seconds)
    }

    /// The gate's Climb step, here without a client: a wolf held at the foot of a step a block
    /// high is struck by the player on the step's edge, then let go with the player three
    /// blocks back along the top. From its first thought once let go (its own beat) its
    /// physics brings it up the step and in reach within `motion::chase_with_hop` (its way at
    /// its run, the hop's time in the air, its gathering speed); it decides to strike at its
    /// first thought in reach, and the blow lands its wind-up later with its feet up on the
    /// step (a blow told the player, a miss as a hit, as the gate counts them).
    #[test]
    fn a_wolf_let_go_below_a_step_comes_up_it_and_strikes_within_its_chase() {
        for salt in 0..12u64 {
            let mut server = crate::test_server();
            server.meta.seed ^= salt * 7919;
            let c = pen(&mut server, 14);
            let stone = server.content.blocks.expect("base:stone");
            for z in 0..=6 {
                for x in -6..=6 {
                    server.world.set_block(c + IVec3::new(x, 0, z), stone);
                }
            }
            let edge = c.as_vec3() + Vec3::new(0.5, 1.0, 0.5);
            let back = edge + Vec3::Z * 3.0;
            // Joined as `join` does, keeping the client's end: every blow at the player is told
            // it.
            let (client, connection) = voxmar_protocol::local_pair();
            let mut joined = crate::player::Player::new(1, connection, edge, 2, server.tick, server.survival.max_health, &server.config.limits);
            joined.joined = true;
            server.players.insert(1, joined);
            let player = 1;
            server.players.get_mut(&player).unwrap().health = traits::Health::full(60);
            // Held where it was put, as `/summon ... held` holds the gate's, and some ticks on
            // (as many as the salt) struck and let go.
            let id = put(&mut server, "base:wolf", c.as_vec3() + Vec3::new(0.5 + salt as f32 * 0.05, 0.0, -1.5), Vec3::Z);
            server.creatures.live.get_mut(&id).unwrap().held_until = server.tick + 60 * TICKS_PER_SECOND as u64;
            for _ in 0..salt {
                server.tick();
            }
            server.hurt_creature(player, id, 2, None, voxmar_engine::combat::Blow::Plain);
            server.players.get_mut(&player).unwrap().position = back;
            server.creatures.live.get_mut(&id).unwrap().held_until = server.tick;
            while client.try_recv().ok().flatten().is_some() {}
            let content = server.content.clone();
            let live = &server.creatures.live[&id];
            let species = content.creatures.get(live.species);
            let (scale, every, from) = (live.scale, NEAR_THINK + live.seed % THINK_SPREAD, server.tick);
            // The ticks from here it thinks on: it sees where it got to by the tick before.
            let beat = |t: u64| (from + t + id as u64).is_multiple_of(every);
            let first = (1..=every).find(|&t| beat(t)).expect("a beat");
            let reach = species.longest_reach(0);
            let way = (species.edge(live.body.position.with_y(0.0).distance(back.with_y(0.0)), scale) - reach).max(0.0);
            let second = TICKS_PER_SECOND as f32;
            let chase = (motion::chase_with_hop(&species.def.movement, way, species.def.movement.run, 1.0 / second) * second).ceil() as u64;
            let windup = (species.def.attack.and_then(|a| a.windup).unwrap_or(content.combat.feel.creature_windup) * second).round() as u64;
            let (mut in_reach, mut decided, mut blow) = (None, None, None);
            for t in 1..=first + chase + every + windup + 20 {
                server.tick();
                let live = &server.creatures.live[&id];
                if in_reach.is_none() && species.reaches(0, species.edge(live.body.position.with_y(0.0).distance(back.with_y(0.0)), scale)) {
                    in_reach = Some(t);
                }
                if decided.is_none() && live.winding {
                    decided = Some(t);
                }
                if std::iter::from_fn(|| client.try_recv().ok().flatten()).any(|m| matches!(m, ServerMessage::Hurt { creature, .. } if creature == id)) {
                    blow = Some((t, live.body.position.y - c.y as f32));
                    break;
                }
            }
            let came = first - 1 + chase;
            let in_reach = in_reach.unwrap_or_else(|| panic!("salt {salt}: never in reach of the player"));
            assert!(in_reach <= came, "salt {salt}: in reach {in_reach} ticks after it was let go, past its first thought in {first} and {chase} for {way:.2} blocks and the hop");
            let thought = (in_reach + 1..).find(|&t| beat(t)).expect("a beat");
            assert_eq!(decided, Some(thought), "salt {salt}: in reach at {in_reach}, it decides to strike at its next thought");
            let (landed, up) = blow.unwrap_or_else(|| panic!("salt {salt}: no blow"));
            assert_eq!(landed, thought + windup, "salt {salt}: the blow lands its wind-up after it decides");
            assert!(up > 0.9, "salt {salt}: the wolf struck from below the step ({up:.2} up)");
        }
    }

    #[test]
    fn hunters_come_round_a_tree_and_up_a_step() {
        let tree = |server: &mut Server, c: IVec3| {
            wall(server, [c], 6);
            c.as_vec3() + Vec3::new(5.5, 0.0, 0.5)
        };
        let step = |server: &mut Server, c: IVec3| {
            let stone = server.content.blocks.expect("base:stone");
            for z in -1..=1 {
                for x in 4..=6 {
                    server.world.set_block(c + IVec3::new(x, 0, z), stone);
                }
            }
            c.as_vec3() + Vec3::new(5.5, 1.0, 0.5)
        };
        // The badger's own jump does not lift it a block. A wild boar only fights back, struck.
        for name in ["base:wolf", "base:brown_bear", "base:badger", "base:wild_boar"] {
            assert!(hunter(name, tree, 6).is_some(), "{name} never came round the tree");
            assert!(hunter(name, step, 6).is_some(), "{name} never came up the step");
        }
    }

    #[test]
    fn a_hunter_finds_the_doorway_in_a_wall_but_not_through_a_shut_door() {
        let doorway = |door: Option<&'static str>| move |server: &mut Server, c: IVec3| {
            walled(server, c, door);
            c.as_vec3() + Vec3::new(5.5, 0.0, 0.5)
        };
        for name in ["base:wolf", "base:badger"] {
            assert!(hunter(name, doorway(None), 8).is_some(), "{name} never found the doorway");
            assert!(hunter(name, doorway(Some("base:wooden_door_open")), 8).is_some(), "{name} never came through an open door");
            assert_eq!(hunter(name, doorway(Some("base:wooden_door")), 8), None, "{name} came through a shut door");
        }
    }

    #[test]
    fn a_sheep_after_wheat_follows_through_a_doorway() {
        let mut server = crate::test_server();
        let c = pen(&mut server, 14);
        walled(&mut server, c, None);
        let to = c.as_vec3() + Vec3::new(5.5, 0.0, 0.5);
        let player = join(&mut server, to);
        let wheat = server.content.blocks.expect("base:wheat");
        let held = server.players.get_mut(&player).unwrap();
        held.inventory.add(wheat, 1);
        held.selected = (0..36).find(|&slot| held.inventory.get(slot).is_some_and(|s| s.item == wheat)).expect("the wheat is held");
        let id = put(&mut server, "base:mouflon", c.as_vec3() + Vec3::new(-3.5, 0.0, 0.5), Vec3::X);
        assert!(comes(&mut server, id, to, 12).is_some(), "the sheep never found its way to the wheat");
    }

    /// Seven junglefowl heaped in the end cell of a lane one block wide and two high, facing
    /// its stone, alarmed by kin (as when the player walked in without food and scared some
    /// off), and the player four blocks off holding wheat seeds. The kiln bot's A Tempting
    /// Bite saw them stay there hopping at the stone, running "home" into the dead end. Food
    /// held up by a player who never hurt them settles them: they turn out of the heap and
    /// walk up, one behind another.
    #[test]
    fn alarmed_fowl_heaped_in_a_dead_end_come_out_to_seeds_held_up() {
        for salt in 0..6u64 {
            let mut server = crate::test_server();
            server.meta.seed ^= salt * 7919;
            let c = pen(&mut server, 14);
            wall(&mut server, (-11..=0).flat_map(|z| [c + IVec3::new(-1, 0, z), c + IVec3::new(1, 0, z)]).chain([c + IVec3::new(0, 0, -11)]).collect::<Vec<_>>(), 2);
            let lane = |z: f32| c.as_vec3() + Vec3::new(0.5, 0.0, z + 0.5);
            let to = lane(-6.0);
            let player = join(&mut server, to);
            let fowl: Vec<u32> = (0..7)
                .map(|i| {
                    let at = lane(-10.0) + Vec3::new((i as f32 * 0.9).sin() * 0.25, 0.0, (i as f32 * 1.7).cos() * 0.25);
                    let id = put(&mut server, "base:junglefowl", at, -Vec3::Z);
                    let live = server.creatures.live.get_mut(&id).unwrap();
                    live.home = at;
                    live.mind.alarmed();
                    id
                })
                .collect();
            let seeds = server.content.blocks.expect("base:wheat_seeds");
            let held = server.players.get_mut(&player).unwrap();
            held.inventory.add(seeds, 4);
            held.selected = (0..36).find(|&slot| held.inventory.get(slot).is_some_and(|s| s.item == seeds)).expect("the seeds are held");
            let near = |server: &Server| fowl.iter().filter(|id| server.creatures.live[id].body.position.distance(to) < 3.0).count();
            let first = (0..2 * TICKS_PER_SECOND as u64).find(|_| {
                server.tick += 1;
                server.think_and_move();
                near(&server) > 0
            });
            let heap = |server: &Server| fowl.iter().map(|id| server.creatures.live[id].body.position - c.as_vec3()).collect::<Vec<_>>();
            assert!(first.is_some(), "no junglefowl came out to the seeds within 2 s (salt {salt}): {:.2?}", heap(&server));
            for _ in 0..5 * TICKS_PER_SECOND as u64 {
                server.tick += 1;
                server.think_and_move();
            }
            let out = fowl.iter().filter(|id| server.creatures.live[id].body.position.z > lane(-9.0).z).count();
            assert!(out >= 5, "only {out} of 7 junglefowl left the end cell after 7 s (salt {salt}): {:.2?}", heap(&server));
        }
    }

    /// A deer struck shut in a box of stone one block across has nowhere to run: it stands
    /// rather than press into the walls, but its stuck clock still runs and it gives up the
    /// goal (for another, or the next look at a way round).
    #[test]
    fn a_creature_boxed_in_gives_up_its_goal() {
        let mut server = crate::test_server();
        let c = pen(&mut server, 6);
        wall(&mut server, [IVec3::new(-1, 0, 0), IVec3::new(1, 0, 0), IVec3::new(0, 0, -1), IVec3::new(0, 0, 1)].map(|d| c + d), 3);
        let player = join(&mut server, c.as_vec3() + Vec3::new(3.5, 0.0, 0.5));
        let id = put(&mut server, "base:rabbit", c.as_vec3() + Vec3::new(0.5, 0.0, 0.5), Vec3::X);
        server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
        let mut most: f32 = 0.0;
        for _ in 0..4 * TICKS_PER_SECOND as u64 {
            server.tick += 1;
            server.think_and_move();
            most = most.max(server.creatures.live[&id].stuck);
        }
        assert!(most > 1.5, "a boxed-in rabbit's stuck clock never ran (most {most:.2})");
    }

    /// The pen's x > 0 half sunk `depth` blocks, with water to its rim when `wet`.
    fn ledge(server: &mut Server, c: IVec3, depth: i32, wet: bool) {
        let (stone, water) = (server.content.blocks.expect("base:stone"), server.content.blocks.expect("base:water"));
        for z in -13..=13 {
            for x in 1..=13 {
                for y in -depth..0 {
                    server.world.set_block(c + IVec3::new(x, y, z), if y == -depth { stone } else if wet { water } else { BlockId::AIR });
                }
            }
        }
    }

    #[test]
    fn prey_runs_along_a_cliff_edge_and_a_lake_shore_not_off_them() {
        for (depth, wet) in [(6, false), (3, true)] {
            let mut server = crate::test_server();
            let c = pen(&mut server, 14);
            ledge(&mut server, c, depth, wet);
            let player = join(&mut server, c.as_vec3() + Vec3::new(-6.5, 0.0, 0.5));
            for name in ["base:mouflon", "base:red_deer", "base:rabbit"] {
                for side in [-3.5, 0.5, 4.5] {
                    let id = put(&mut server, name, c.as_vec3() + Vec3::new(-1.5, 0.0, side), Vec3::X);
                    server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
                    for t in 0..6 * TICKS_PER_SECOND as u64 {
                        server.tick += 1;
                        server.think_and_move();
                        let at = server.creatures.live[&id].body.position;
                        assert!(at.y > c.y as f32 - 0.5, "{name} went over the {} at {} (tick {t})", if wet { "shore" } else { "cliff" }, at - c.as_vec3());
                    }
                    server.creatures.live.remove(&id);
                }
            }
        }
    }

    #[test]
    fn creatures_are_made_facing_open_air_among_trunks() {
        let mut server = crate::test_server();
        let c = pen(&mut server, 14);
        for x in (-12..=12).step_by(2) {
            for z in (-12..=12).step_by(2) {
                wall(&mut server, [c + IVec3::new(x, 0, z)], 4);
            }
        }
        let deer = server.content.creatures.id("base:red_deer").expect("deer");
        for i in 0..60u64 {
            let at = c.as_vec3() + Vec3::new((i % 12) as f32 * 2.0 - 11.5, 0.0, (i / 12) as f32 * 2.0 - 4.5);
            let id = server.spawn_creature(deer, at, i).expect("room");
            server.move_creature(id, 0.0);
            assert!(!server.creatures.live[&id].body.head_blocked(&Ground::of(&server)), "a deer was made with its head in a trunk at {}", at - c.as_vec3());
        }
    }
}


/// Animals at rest in a herd, after food held up, and out of their depth: a herd keeps
/// together without twitching to and fro, followers keep up and do not crowd the player,
/// and a land animal in deep water makes for the shore.
#[cfg(test)]
mod herd_tests {
    use super::*;
    use super::cornered_tests::{join, pen, put, wall};

    /// Short starts (moving under 0.75 s between stands), sharp turns (over 90 degrees in
    /// half a second, moving) and the widest the herd got, of six `name` left to themselves
    /// a minute, over four worlds.
    fn herd_jitter(name: &str) -> (u32, u32, f32) {
        let (mut short, mut sharp, mut widest) = (0, 0, 0.0f32);
        for salt in 0..4u64 {
            let mut server = crate::test_server();
            server.meta.seed ^= salt * 7919;
            let c = pen(&mut server, 14);
            let ids: Vec<u32> = (0..6).map(|i| put(&mut server, name, c.as_vec3() + Vec3::new((i % 3) as f32 * 1.2 - 1.2, 0.0, (i / 3) as f32 * 1.2), Vec3::X)).collect();
            let mut runs = vec![0u32; ids.len()];
            let mut heads: Vec<std::collections::VecDeque<Option<Vec3>>> = vec![Default::default(); ids.len()];
            for _ in 0..60 * TICKS_PER_SECOND as u64 {
                server.tick += 1;
                let before: Vec<Vec3> = ids.iter().map(|id| server.creatures.live[id].body.position).collect();
                server.think_and_move();
                for (k, id) in ids.iter().enumerate() {
                    let step = (server.creatures.live[id].body.position - before[k]).with_y(0.0);
                    let moving = step.length() * TICKS_PER_SECOND as f32 > 0.3;
                    if moving {
                        runs[k] += 1;
                    } else {
                        short += (runs[k] > 0 && runs[k] < 15) as u32;
                        runs[k] = 0;
                    }
                    heads[k].push_back(moving.then(|| step.normalize()));
                    if heads[k].len() > 10 && let (Some(Some(a)), Some(Some(b))) = (heads[k].pop_front(), heads[k].back().copied()) && a.dot(b) < 0.0 {
                        sharp += 1;
                        heads[k].clear();
                    }
                }
            }
            let at: Vec<Vec3> = ids.iter().map(|id| server.creatures.live[id].body.position).collect();
            widest = widest.max(at.iter().flat_map(|a| at.iter().map(move |b| a.distance(*b))).fold(0.0, f32::max));
        }
        (short, sharp, widest)
    }

    /// What a creature thinks is drawn by its own count of thoughts, never the tick it
    /// thinks on: the same herd let be from a tick far on wanders alike.
    #[test]
    fn a_creatures_thoughts_draw_by_its_own_count_not_the_tick() {
        let wandered = |from: u64| {
            let mut server = crate::test_server();
            let c = pen(&mut server, 14);
            let ids: Vec<u32> = (0..3).map(|i| put(&mut server, "base:mouflon", c.as_vec3() + Vec3::new(i as f32 * 1.5 - 1.5, 0.0, 0.0), Vec3::X)).collect();
            server.tick = from;
            for _ in 0..20 * TICKS_PER_SECOND as u64 {
                server.tick += 1;
                server.think_and_move();
            }
            ids.iter().map(|id| (server.creatures.live[id].rolls, server.creatures.live[id].body.position.to_array())).collect::<Vec<_>>()
        };
        // A whole number of every beat a creature may think at on (`NEAR_THINK`,
        // `FAR_THINK`, each spread over `THINK_SPREAD`), so each thinks on the same ticks of its own.
        let beats: u64 = (0..THINK_SPREAD).map(|k| NEAR_THINK + k).chain((0..THINK_SPREAD).map(|k| FAR_THINK + k)).product();
        let first = wandered(1_000);
        assert!(first.iter().all(|(rolls, _)| *rolls > 3), "they hardly thought: {first:?}");
        assert_eq!(wandered(1_000 + 7 * beats), first, "the same herd from later thought otherwise");
    }

    /// The start of a fixed-seed creature's own count is seen in how it moves, so it is
    /// cast by the cast key on the world's count, never drawn by the draw key.
    #[test]
    fn a_creatures_count_starts_from_the_cast_key() {
        use crate::save::{ROLL_START, cast_key, numbered_draws};
        let mut server = crate::test_server();
        let c = pen(&mut server, 14);
        let id = put(&mut server, "base:mouflon", c.as_vec3(), Vec3::X);
        server.meta.draw_key = 0x0d1a;
        server.meta.rolls = 41;
        let bits = |[a, b]: [f32; 2]| u64::from(a.to_bits()) << 32 | u64::from(b.to_bits());
        server.start_rolls(id);
        assert_eq!(server.creatures.live[&id].rolls, bits(numbered_draws(cast_key(0x0d1a), ROLL_START, 41)));
        assert_ne!(server.creatures.live[&id].rolls, bits(numbered_draws(0x0d1a, ROLL_START, 41)), "the count started from the draw key");
        assert_eq!(server.meta.rolls, 42);
    }

    /// A creature's thoughts and products draw by its count with salts of their own nested
    /// round it, so no count's thought is another count's product, or the same seed's.
    #[test]
    fn thoughts_and_products_never_draw_alike_at_any_count() {
        let thought = |seed: u64, n: u64| own_roll(seed, n, THOUGHT);
        let product = |seed: u64, n: u64| own_roll(seed, n, PRODUCT);
        for seed in [0, 1, 0x5eed, u64::MAX] {
            for n in [0, 1, 7, 600, 1 << 40] {
                assert_ne!(thought(seed, n), product(seed, n ^ THOUGHT ^ PRODUCT), "seed {seed:x} count {n}: a thought is a product");
                assert_ne!(thought(seed, n), product(seed, n), "seed {seed:x} count {n}");
                assert_ne!(thought(seed, n), thought(seed, n + 1), "seed {seed:x} count {n}: two counts think alike");
                assert_ne!(thought(seed, n), seed, "seed {seed:x} count {n}");
            }
        }
    }

    /// Before: a herd's will to keep together was drawn afresh each tick, its companion was
    /// whichever the list of those about put first, two near each other went round to each
    /// other's far side, and strolls took them well off: 288 sharp turns for mouflon, 256
    /// for red deer, 394 for junglefowl.
    #[test]
    fn a_herd_at_rest_keeps_together_without_turning_to_and_fro() {
        for (name, most) in [("base:mouflon", 110), ("base:red_deer", 110), ("base:junglefowl", 220)] {
            let (short, sharp, widest) = herd_jitter(name);
            assert!(sharp <= most, "{name}: {sharp} sharp turns in six animals over four minutes (at most {most})");
            assert!(short <= 70, "{name}: {short} short starts");
            assert!(widest < 15.0, "{name}: the herd spread {widest:.1} blocks");
        }
    }

    /// Three kept animals of each kind after food their owner holds, who walks off through
    /// a gap a block wide and two high in a wall and on. Before: they walked slower than the
    /// player, lost the food at ten blocks and walked back home, and rabbits hopped in under
    /// the player's feet.
    #[test]
    fn kept_animals_follow_held_food_through_a_gap_keep_up_and_stay_where_led() {
        for (name, food) in [("base:mouflon", "base:wheat"), ("base:red_deer", "base:apple"), ("base:junglefowl", "base:wheat_seeds"), ("base:rabbit", "base:carrot")] {
            let mut server = crate::test_server();
            let c = pen(&mut server, 14);
            wall(&mut server, (-13..=13).filter(|&z| z != 0).map(|z| c + IVec3::new(0, 0, z)).collect::<Vec<_>>(), 2);
            let stone = server.content.blocks.expect("base:stone");
            server.world.set_block(c + IVec3::new(0, 2, 0), stone);
            let player = join(&mut server, c.as_vec3() + Vec3::new(-6.5, 0.0, 0.5));
            let item = server.content.blocks.expect(food);
            let owner = server.players[&player].name.clone();
            let held = server.players.get_mut(&player).unwrap();
            held.inventory.add(item, 4);
            held.selected = (0..36).find(|&slot| held.inventory.get(slot).is_some_and(|s| s.item == item)).expect("the food is held");
            let ids: Vec<u32> = (0..3)
                .map(|i| {
                    let id = put(&mut server, name, c.as_vec3() + Vec3::new(-8.5, 0.0, i as f32 * 1.5 - 1.0), Vec3::X);
                    server.creatures.live.get_mut(&id).unwrap().owner = Some(owner.clone());
                    id
                })
                .collect();
            // Through the gap at a walk, and on.
            let route = [Vec3::new(-1.5, 0.0, 0.5), Vec3::new(1.5, 0.0, 0.5), Vec3::new(9.5, 0.0, 0.5)].map(|r| c.as_vec3() + r);
            let (mut leg, mut crowding, mut stalled) = (0, 0, 0);
            let mut last: Vec<Vec3> = ids.iter().map(|id| server.creatures.live[id].body.position).collect();
            for t in 0..20 * TICKS_PER_SECOND as u64 {
                server.tick += 1;
                let p = server.players[&player].position;
                if let Some(&to) = route.get(leg) {
                    let step = (to - p).with_y(0.0);
                    if step.length() < 0.1 {
                        leg += 1;
                    } else {
                        server.players.get_mut(&player).unwrap().position = p + step.normalize() * step.length().min(3.0 / TICKS_PER_SECOND as f32);
                    }
                }
                server.think_and_move();
                let p = server.players[&player].position;
                for (k, id) in ids.iter().enumerate() {
                    let at = server.creatures.live[id].body.position;
                    crowding += (flat_distance(at, p) < 1.2) as u32;
                    if t % TICKS_PER_SECOND as u64 == 0 {
                        stalled += ((at - last[k]).with_y(0.0).length() < 0.2 && at.distance(p) > 3.5) as u32;
                        last[k] = at;
                    }
                }
            }
            let p = server.players[&player].position;
            let at = |server: &Server| ids.iter().map(|id| server.creatures.live[id].body.position - c.as_vec3()).collect::<Vec<_>>();
            assert_eq!(stalled, 0, "{name}: stood still {stalled} seconds behind the player: {:.1?}", at(&server));
            assert!(crowding <= 40, "{name}: {crowding} ticks under the player's feet");
            assert!(ids.iter().all(|id| flat_distance(server.creatures.live[id].body.position, p) < 4.0), "{name}: not all came along: {:.1?}", at(&server));
            // The food put away, they stay where they were led.
            server.players.get_mut(&player).unwrap().selected = (0..36).find(|&slot| server.players[&player].inventory.get(slot).is_none()).expect("an empty slot");
            for _ in 0..15 * TICKS_PER_SECOND as u64 {
                server.tick += 1;
                server.think_and_move();
            }
            assert!(ids.iter().all(|id| server.creatures.live[id].body.position.x > c.x as f32 + 2.0), "{name}: walked back to where they were: {:.1?}", at(&server));
        }
    }

    fn flat_distance(a: Vec3, b: Vec3) -> f32 {
        a.with_y(0.0).distance(b.with_y(0.0))
    }

    /// A lake two deep, its east shore 4.5 blocks off: each kind swims for it and climbs
    /// out. Before: they paddled wherever they had been going, 9 to 33 s, a wolf never.
    #[test]
    fn a_land_animal_out_of_its_depth_swims_to_the_nearest_shore() {
        for name in ["base:mouflon", "base:red_deer", "base:rabbit", "base:junglefowl", "base:wolf", "base:badger"] {
            let mut server = crate::test_server();
            let c = pen(&mut server, 14);
            let (stone, water) = (server.content.blocks.expect("base:stone"), server.content.blocks.expect("base:water"));
            for z in -6..=6 {
                for x in -6..=6 {
                    server.world.set_block(c + IVec3::new(x, -1, z), water);
                    server.world.set_block(c + IVec3::new(x, -2, z), water);
                    server.world.set_block(c + IVec3::new(x, -3, z), stone);
                }
            }
            let id = put(&mut server, name, c.as_vec3() + Vec3::new(2.5, -1.5, 0.5), -Vec3::X);
            let out = (0..8 * TICKS_PER_SECOND as u64).find(|_| {
                server.tick += 1;
                server.think_and_move();
                let body = &server.creatures.live[&id].body;
                body.on_ground && !body.in_water && body.position.y >= c.y as f32 - 0.01
            });
            let at = server.creatures.live[&id].body.position - c.as_vec3();
            assert!(out.is_some(), "{name} was still in the lake after 8 s, at {at:.2}");
            assert!(at.x > 6.5, "{name} came out on a far shore, at {at:.2}");
        }
    }

    /// A lake two deep shelving to a block deep before its east shore: once out of the deep
    /// water, each kind keeps on through the shallows to the shore, and does not wade back
    /// into the deep after.
    #[test]
    fn a_land_animal_wades_out_through_shallows_and_keeps_out_of_the_deep() {
        for name in ["base:mouflon", "base:red_deer", "base:rabbit", "base:junglefowl", "base:wolf", "base:badger"] {
            for salt in 0..3u64 {
                let mut server = crate::test_server();
                server.meta.seed ^= salt * 7919;
                let c = pen(&mut server, 14);
                let (stone, water) = (server.content.blocks.expect("base:stone"), server.content.blocks.expect("base:water"));
                for z in -8..=8 {
                    for x in -8..=8 {
                        let shallow = x >= 2;
                        server.world.set_block(c + IVec3::new(x, -1, z), water);
                        server.world.set_block(c + IVec3::new(x, -2, z), if shallow { stone } else { water });
                        server.world.set_block(c + IVec3::new(x, -3, z), stone);
                    }
                }
                let id = put(&mut server, name, c.as_vec3() + Vec3::new(-1.5, -1.5, 0.5), Vec3::X);
                // In the lake below the bank's top (as `out` is above it), over water: one on
                // the bank whose box only overhangs the lake's edge is not in the deep.
                let deep = |server: &Server| {
                    let body = &server.creatures.live[&id].body;
                    body.in_water && body.position.y < c.y as f32 - 0.01 && Ground::of(server).water_at(body.position.floor().as_ivec3() - IVec3::Y)
                };
                let out = (0..12 * TICKS_PER_SECOND as u64).find(|_| {
                    server.tick += 1;
                    server.think_and_move();
                    let body = &server.creatures.live[&id].body;
                    body.on_ground && !body.in_water && body.position.y >= c.y as f32 - 0.01
                });
                let at = server.creatures.live[&id].body.position - c.as_vec3();
                assert!(out.is_some(), "{name} (world {salt}) was still in the lake after 12 s, at {at:.2}");
                for t in 0..30 * TICKS_PER_SECOND as u64 {
                    server.tick += 1;
                    server.think_and_move();
                    let at = server.creatures.live[&id].body.position - c.as_vec3();
                    assert!(!deep(&server), "{name} (world {salt}) waded back into the deep water at {at:.2} (tick {t})");
                }
            }
        }
    }
}

/// Chases and flights when a player attacks: prey runs over open ground (not into the
/// fence, the tree or the pond), hunters come round them, and a hunter that cannot get at
/// the player, or would follow past its leash, gives up and goes home.
#[cfg(test)]
mod chase_tests {
    use super::*;
    use super::cornered_tests::{badly_hurt, join, pen, put, wall};

    fn doing(server: &Server, id: u32) -> &'static str {
        let c = &server.creatures.live[&id];
        c.mind.doing.map_or("-", |i| server.content.creatures.get(c.species).modules[i].0.name())
    }

    /// A fence along x = 3 for z -8..8 (two high), a tree at (-3, 4), a pond two deep at
    /// x -8..-5, z -3..1.
    fn yard(server: &mut Server, c: IVec3) {
        wall(server, (-8..=8).map(|z| c + IVec3::new(3, 0, z)).collect::<Vec<_>>(), 2);
        wall(server, [c + IVec3::new(-3, 0, 4)], 4);
        let (stone, water) = (server.content.blocks.expect("base:stone"), server.content.blocks.expect("base:water"));
        for z in -3..=1 {
            for x in -8..=-5 {
                server.world.set_block(c + IVec3::new(x, -1, z), water);
                server.world.set_block(c + IVec3::new(x, -2, z), water);
                server.world.set_block(c + IVec3::new(x, -3, z), stone);
            }
        }
    }

    fn heal(server: &mut Server, player: PlayerId) {
        let health = &mut server.players.get_mut(&player).unwrap().health;
        health.current = health.max;
    }

    /// Whether a body moved under a fifth of its run this tick while it had a goal.
    fn stalled(server: &Server, id: u32, before: Vec3) -> bool {
        let live = &server.creatures.live[&id];
        let run = server.content.creatures.get(live.species).def.movement.run;
        live.decision.goal.is_some() && (live.body.position - before).with_y(0.0).length() < run / TICKS_PER_SECOND as f32 * 0.2
    }

    /// Prey struck beside the fence, the tree, the pond, and with its way off toward them:
    /// six seconds each, they never go into the water, stall little and touch little.
    #[test]
    fn struck_prey_runs_over_open_ground_past_fence_tree_and_pond() {
        let starts = [((1.5, 0.5), (-1.0, 0.0)), ((-1.5, 4.5), (-1.0, 0.0)), ((-3.5, -1.5), (1.0, 0.0)), ((1.5, -6.5), (-1.0, 0.3))];
        for name in ["base:rabbit", "base:red_deer", "base:mouflon", "base:junglefowl", "base:wild_boar"] {
            let (mut stalls, mut touching, mut gained) = (0, 0, 0.0);
            for &((x, z), (ax, az)) in &starts {
                let mut server = crate::test_server();
                let c = pen(&mut server, 30);
                yard(&mut server, c);
                let at = c.as_vec3() + Vec3::new(x, 0.0, z);
                let player = join(&mut server, at + Vec3::new(ax, 0.0, az).normalize() * 1.5);
                let id = put(&mut server, name, at, -Vec3::new(ax, 0.0, az));
                badly_hurt(&mut server, id);
                server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
                for _ in 0..6 * TICKS_PER_SECOND as u64 {
                    server.tick += 1;
                    let before = server.creatures.live[&id].body.position;
                    server.think_and_move();
                    stalls += stalled(&server, id, before) as u32;
                    let body = &server.creatures.live[&id].body;
                    touching += body.against_wall as u32;
                    assert!(!body.in_water, "{name} ran into the pond from ({x}, {z}), at {:.1}", body.position - c.as_vec3());
                }
                gained += flat(server.creatures.live[&id].body.position, server.players[&player].position);
            }
            let mean = gained / starts.len() as f32;
            assert!(stalls <= 60, "{name}: stalled {stalls} ticks of 480");
            assert!(touching <= 40, "{name}: against a block {touching} ticks of 480");
            assert!(mean > 18.0, "{name}: got only {mean:.1} blocks clear in six seconds");
        }
    }

    fn flat(a: Vec3, b: Vec3) -> f32 {
        a.with_y(0.0).distance(b.with_y(0.0))
    }

    /// The player behind the fence (the way round at its ends), behind the tree, across
    /// the pond: an angered hunter comes round to them, stalling and turning back little.
    #[test]
    fn struck_hunters_come_round_fence_tree_and_pond() {
        let scenes = [((6.5, 0.5), (-0.5, 0.5)), ((-3.5, 7.5), (-3.5, 0.5)), ((-6.5, 4.5), (-6.5, -6.5))];
        for name in ["base:wolf", "base:brown_bear", "base:badger", "base:wild_boar"] {
            for &((px, pz), (hx, hz)) in &scenes {
                let mut server = crate::test_server();
                let c = pen(&mut server, 14);
                yard(&mut server, c);
                let player = join(&mut server, c.as_vec3() + Vec3::new(px, 0.0, pz));
                let id = put(&mut server, name, c.as_vec3() + Vec3::new(hx, 0.0, hz), Vec3::X);
                server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
                let (mut stalls, mut back) = (0, 0);
                let mut heads: std::collections::VecDeque<Option<Vec3>> = Default::default();
                let p = server.players[&player].position;
                let came = (0..8 * TICKS_PER_SECOND as u64).find(|_| {
                    server.tick += 1;
                    let before = server.creatures.live[&id].body.position;
                    server.think_and_move();
                    heal(&mut server, player);
                    stalls += stalled(&server, id, before) as u32;
                    let step = (server.creatures.live[&id].body.position - before).with_y(0.0);
                    heads.push_back((step.length() * TICKS_PER_SECOND as f32 > 0.3).then(|| step.normalize()));
                    if heads.len() > 10 && let (Some(Some(a)), Some(Some(b))) = (heads.pop_front(), heads.back().copied()) && a.dot(b) < 0.0 {
                        back += 1;
                        heads.clear();
                    }
                    flat(server.creatures.live[&id].body.position, p) < 2.0
                });
                assert!(came.is_some(), "{name} never came round to the player at ({px}, {pz}): {:.1}", server.creatures.live[&id].body.position - c.as_vec3());
                assert!(stalls <= 10 && back <= 3, "{name} to ({px}, {pz}): stalled {stalls} ticks, turned back {back} times");
            }
        }
    }

    /// The player shut in a stone box, struck at by a hunter from outside it. Before: wolf,
    /// bear and badger hung about the box 32 to 37 seconds of 40. Now no way in found, they
    /// balk twice, give up and lope home (5, 7 and 10 s by the box), not stand by it staring
    /// or spinning to face the player through its wall, nor come back to it while they
    /// remember giving up; struck again, they take the fight up again.
    #[test]
    fn a_hunter_that_cannot_get_at_the_player_gives_up_and_goes_home() {
        for name in ["base:wolf", "base:brown_bear", "base:badger"] {
            let mut server = crate::test_server();
            let c = pen(&mut server, 14);
            wall(&mut server, (-2..=2).flat_map(|i| [IVec3::new(i, 0, -2), IVec3::new(i, 0, 2), IVec3::new(-2, 0, i), IVec3::new(2, 0, i)]).map(|d| c + d).collect::<Vec<_>>(), 3);
            let player = join(&mut server, c.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
            let home = c.as_vec3() + Vec3::new(-9.5, 0.0, 0.5);
            let id = put(&mut server, name, home, Vec3::X);
            server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
            let (mut hanging, mut back) = (0, f32::MAX);
            for _ in 0..40 * TICKS_PER_SECOND as u64 {
                server.tick += 1;
                server.think_and_move();
                heal(&mut server, player);
                let at = server.creatures.live[&id].body.position;
                hanging += (flat(at, c.as_vec3()) < 4.5) as u32;
                if hanging > 0 {
                    back = back.min(flat(at, home));
                }
            }
            let seconds = hanging as f32 / TICKS_PER_SECOND as f32;
            assert!(seconds <= 13.0, "{name} hung about the box {seconds:.1} s of 40");
            assert!(back < 4.0, "{name} never went home: {back:.1} blocks off it at nearest");
            assert!(!matches!(doing(&server, id), "hunt" | "defend"), "{name} was still after the player");
            server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
            for _ in 0..TICKS_PER_SECOND as u64 {
                server.tick += 1;
                server.think_and_move();
            }
            assert!(matches!(doing(&server, id), "hunt" | "defend"), "{name} struck again did not take the fight up: {}", doing(&server, id));
        }
    }

    /// A player walking off across open land a little faster than the hunter runs. Before:
    /// they followed 105 blocks (leashes 20 to 48). Now they turn back at their leash.
    #[test]
    fn a_hunter_follows_no_further_than_its_leash() {
        for name in ["base:wolf", "base:brown_bear", "base:badger"] {
            let mut server = crate::test_server();
            let c = pen(&mut server, 60);
            let home = c.as_vec3() + Vec3::new(-50.5, 0.0, 0.5);
            let player = join(&mut server, home + Vec3::new(2.0, 0.0, 0.0));
            let id = put(&mut server, name, home, Vec3::X);
            server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
            let def = &server.content.creatures.get(server.creatures.live[&id].species).def;
            let (run, leash) = (def.movement.run, def.leash);
            let mut furthest = 0.0f32;
            for _ in 0..30 * TICKS_PER_SECOND as u64 {
                server.tick += 1;
                let p = server.players[&player].position;
                if p.x < c.x as f32 + 55.0 {
                    server.players.get_mut(&player).unwrap().position = p + Vec3::X * run * 1.05 / TICKS_PER_SECOND as f32;
                }
                server.think_and_move();
                heal(&mut server, player);
                furthest = furthest.max(flat(server.creatures.live[&id].body.position, home));
            }
            let end = flat(server.creatures.live[&id].body.position, home);
            assert!(furthest <= leash + 3.0, "{name} followed {furthest:.1} blocks from home (leash {leash})");
            assert!(end < leash, "{name} did not turn back home: {end:.1} blocks off, {}", doing(&server, id));
        }
    }

    /// A player who led a hunter past its leash and stands to fight it there, striking it
    /// every second. Before: it gave up at every think and never struck back. Now it fights.
    #[test]
    fn a_hunter_past_its_leash_fights_a_player_who_stands_and_strikes_it() {
        for name in ["base:wolf", "base:brown_bear", "base:badger"] {
            let mut server = crate::test_server();
            let c = pen(&mut server, 12);
            let at = c.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
            let player = join(&mut server, at + Vec3::new(1.6, 0.0, 0.0));
            let id = put(&mut server, name, at, Vec3::X);
            let leash = server.content.creatures.get(server.creatures.live[&id].species).def.leash;
            server.creatures.live.get_mut(&id).unwrap().home = at - Vec3::X * (leash + 10.0);
            let mut struck = false;
            for t in 0..6 * TICKS_PER_SECOND as u64 {
                server.tick += 1;
                if t % TICKS_PER_SECOND as u64 == 0 {
                    server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
                }
                server.think_and_move();
                heal(&mut server, player);
                struck |= server.creatures.live[&id].decision.attack.is_some();
            }
            assert!(struck, "{name} never struck back past its leash: {}", doing(&server, id));
        }
    }

    /// The same, but bleeding from a flourish with the player gone: bleeding is no blow, and
    /// it goes home. Before: each bleed tick counted as a fresh blow and it chased on.
    #[test]
    fn a_hunter_past_its_leash_bleeding_goes_home() {
        for name in ["base:wolf", "base:brown_bear", "base:badger"] {
            let mut server = crate::test_server();
            let c = pen(&mut server, 12);
            let at = c.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
            let player = join(&mut server, at + Vec3::new(1.6, 0.0, 0.0));
            let id = put(&mut server, name, at, Vec3::X);
            let leash = server.content.creatures.get(server.creatures.live[&id].species).def.leash;
            server.creatures.live.get_mut(&id).unwrap().home = at - Vec3::X * (leash + 10.0);
            server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
            // The player steps back out of its reach and on; it bleeds each second.
            server.players.get_mut(&player).unwrap().position = at + Vec3::new(9.0, 0.0, 0.0);
            for t in 0..8 * TICKS_PER_SECOND as u64 {
                server.tick += 1;
                if t % TICKS_PER_SECOND as u64 == 0 {
                    server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Bleed);
                }
                server.think_and_move();
                heal(&mut server, player);
            }
            let doing = doing(&server, id);
            assert!(doing.starts_with("return_home"), "{name} bleeding past its leash is still at it: {doing}");
        }
    }
}

#[cfg(test)]
mod lure_tests {
    use super::*;

    /// A wild rabbit near a player holding wild carrot counts once as lured, and not while
    /// the player holds something else or once it is kept.
    #[test]
    fn a_favourite_food_held_close_counts_a_creature_lured_once() {
        let mut server = crate::test_server();
        let (_client, connection) = voxmar_protocol::local_pair();
        let player = 1;
        let mut joined = crate::player::Player::new(player, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        joined.joined = true;
        server.players.insert(player, joined);
        let rabbit = server.content.creatures.id("base:rabbit").expect("rabbits");
        let (near, kept, far) = [1.5, -1.5, 6.0].map(|dx| server.spawn_creature(rabbit, server.spawn + Vec3::X * dx, 1).expect("room")).into();
        server.creatures.live.get_mut(&kept).unwrap().owner = Some("test".into());
        let lured = |server: &Server| server.players[&player].tallies.get("lured:base:rabbit").copied().unwrap_or(0);
        let second = |server: &mut Server| {
            server.tick += TICKS_PER_SECOND as u64;
            server.note_lured();
        };
        // A stick in hand tempts nothing.
        server.players.get_mut(&player).unwrap().inventory.slots[0] = Some(ItemStack::new(server.content.blocks.expect("base:stick"), 1));
        second(&mut server);
        assert_eq!(lured(&server), 0, "a stick lured a rabbit");
        server.players.get_mut(&player).unwrap().inventory.slots[0] = Some(ItemStack::new(server.content.blocks.expect("base:wild_carrot"), 1));
        second(&mut server);
        assert_eq!(lured(&server), 1, "only the wild rabbit in reach is lured");
        assert!(server.creatures.live[&near].lured && !server.creatures.live[&far].lured);
        second(&mut server);
        assert_eq!(lured(&server), 1, "the same rabbit counted twice");
    }
}

/// What creatures cost a server tick: 64 kept animals in a pen by a player's home, and the
/// wild creatures about a player in a forest, on a plain and by a pond. Prints server ms per
/// tick and path searches per tick (`cargo test --release -p voxmar_server creature_tick_cost
/// -- --ignored --nocapture`). The numbers compare one build with another on one machine;
/// they say nothing of how fast any machine is.
#[cfg(test)]
pub(crate) mod tick_cost_tests {
    use super::cornered_tests::{join, pen};
    use super::*;
    use std::time::{Duration, Instant};

    thread_local! {
        /// Path searches, and those that found no way, since the thread began.
        pub(crate) static SEARCHES: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
    }

    /// Runs `seconds` of creature ticks, the steps of `tick_creatures` timed one by one;
    /// prints mean, 99th-percentile and worst ms per tick, the mean of each step, and path
    /// searches per tick.
    fn measure(server: &mut Server, label: &str, seconds: u64) {
        let ticks = seconds * TICKS_PER_SECOND as u64;
        let mut all = Vec::new();
        let mut steps = [Duration::ZERO; 6];
        let before = SEARCHES.with(|n| n.get());
        for _ in 0..ticks {
            server.tick += 1;
            let start = Instant::now();
            let mut lap = start;
            let mut mark = |i: usize, steps: &mut [Duration; 6]| {
                let now = Instant::now();
                steps[i] += now - lap;
                lap = now;
            };
            // As `tick_creatures` does.
            server.settle_players();
            if server.tick.is_multiple_of(server.spawn_every_ticks()) {
                let ids: Vec<PlayerId> = server.players.values().filter(|p| p.joined).map(|p| p.id).collect();
                for (i, id) in ids.into_iter().enumerate() {
                    server.spawn_near(id, i as u64);
                }
                server.despawn_far();
            }
            mark(0, &mut steps);
            server.tick_villagers();
            server.tick_wandering();
            mark(1, &mut steps);
            server.think_and_move();
            mark(2, &mut steps);
            server.note_lured();
            server.tick_husbandry();
            mark(3, &mut steps);
            server.drop_fallen_loot();
            if server.tick.is_multiple_of(10 * TICKS_PER_SECOND as u64) {
                server.place_unplaced();
            }
            mark(4, &mut steps);
            server.replicate_creatures();
            mark(5, &mut steps);
            all.push(start.elapsed());
        }
        let after = SEARCHES.with(|n| n.get());
        all.sort();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let mean = all.iter().map(|d| ms(*d)).sum::<f64>() / all.len() as f64;
        let creatures = server.creatures.live.len();
        let per = |d: Duration| ms(d) / ticks as f64;
        // Where every creature ended up, and what it was doing: the same in two builds when
        // a change to what a tick costs changed nothing they do.
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for (id, c) in &server.creatures.live {
            for v in [*id as u64, c.body.position.x.to_bits() as u64, c.body.position.y.to_bits() as u64, c.body.position.z.to_bits() as u64, c.body.yaw.to_bits() as u64, c.mind.doing.map_or(99, |i| i as u64)] {
                hash = (hash ^ v).wrapping_mul(0x100_0000_01b3);
            }
        }
        println!(
            "{label:>9}: {creatures:>3} creatures, state {hash:016x} | ms/tick mean {mean:.3} p99 {:.3} max {:.3} | spawn {:.3} people {:.3} think+move {:.3} lure+keep {:.3} loot {:.3} send {:.3} | searches/tick {:.3} (no way {:.3})",
            ms(all[all.len() * 99 / 100]),
            ms(*all.last().unwrap()),
            per(steps[0]),
            per(steps[1]),
            per(steps[2]),
            per(steps[3]),
            per(steps[4]),
            per(steps[5]),
            (after.0 - before.0) as f64 / ticks as f64,
            (after.1 - before.1) as f64 / ticks as f64,
        );
    }

    /// 64 kept animals of five kinds in a 25-block pen, the player at its gate.
    fn kept_pen(server: &mut Server) {
        let c = pen(server, 12);
        let player = join(server, c.as_vec3() + Vec3::new(10.5, 0.0, 0.5));
        let name = server.players[&player].name.clone();
        let kinds = ["base:mouflon", "base:aurochs", "base:junglefowl", "base:wild_boar", "base:rabbit"];
        for i in 0..64u32 {
            let species = server.content.creatures.id(kinds[i as usize % kinds.len()]).expect("species");
            let at = c.as_vec3() + Vec3::new((i % 8) as f32 * 2.5 - 9.0, 0.0, (i / 8) as f32 * 2.5 - 9.0);
            let id = server.spawn_creature(species, at, i as u64).expect("room");
            let creature = server.creatures.live.get_mut(&id).unwrap();
            creature.owner = Some(name.clone());
            creature.home = c.as_vec3();
        }
    }

    /// A player standing in `biome` (or, for "pond", on the shore of the nearest water),
    /// the world about them filled with its wild creatures over three minutes.
    fn wild(server: &mut Server, biome: &str) -> bool {
        let spot = if biome == "pond" { pond(server) } else { super::density_tests::find_biome(server, biome) };
        let Some(spot) = spot else { return false };
        let noon = (0..24_000u64).step_by(100).max_by(|a, b| creature::daylight(*a).total_cmp(&creature::daylight(*b))).unwrap();
        super::density_tests::density(server, spot, noon + super::density_tests::LATER, 3, false);
        true
    }

    /// The nearest dry site beside a lake or river (not the sea) to the world's origin, in
    /// 16-block steps.
    fn pond(server: &Server) -> Option<(i32, i32)> {
        let wet = |x: i32, z: i32| {
            let site = server.overworld.site(x, z);
            site.water > site.height
        };
        (1..80).find_map(|r| {
            (0..8 * r).find_map(|i| {
                let (x, z) = ring(r, i);
                let (x, z) = (x * 16, z * 16);
                let inland = server.overworld.site(x, z).height > voxmar_worldgen::SEA_LEVEL + 2;
                (inland && !wet(x, z) && [(6, 0), (-6, 0), (0, 6), (0, -6)].iter().any(|(dx, dz)| wet(x + dx, z + dz))).then_some((x, z))
            })
        })
    }

    #[test]
    #[ignore]
    fn creature_tick_cost() {
        // VOXMAR_COST names one case to run alone (for a profiler).
        let only = std::env::var("VOXMAR_COST").ok();
        let runs = |case: &str| only.as_deref().is_none_or(|o| o == case);
        if runs("kept") {
            let mut server = crate::test_server();
            kept_pen(&mut server);
            measure(&mut server, "kept pen", 60);
        }
        for biome in ["forest", "plains", "pond"].into_iter().filter(|b| runs(b)) {
            let mut server = crate::test_server();
            if !wild(&mut server, biome) {
                println!("{biome:>14}: none found");
                continue;
            }
            measure(&mut server, biome, 60);
        }
    }

    /// Counts the block lookups a search makes.
    struct Counted<'a>(Ground<'a>, std::cell::Cell<u64>);

    impl motion::Surroundings for Counted<'_> {
        fn solid(&self, c: IVec3) -> bool {
            self.1.set(self.1.get() + 1);
            self.0.solid(c)
        }
        fn water(&self, c: IVec3) -> bool {
            self.1.set(self.1.get() + 1);
            self.0.water(c)
        }
        fn footing(&self, c: IVec3) -> bool {
            self.1.set(self.1.get() + 1);
            self.0.footing(c)
        }
    }

    /// Block lookups and time per path search on the server's own ground, in a pen with a
    /// wall across it: no doorway (a search that finds nothing), one a block across (none
    /// for a body two across) and one two across; for a body a block across and one two.
    /// `cargo test -p voxmar_server --release search_cost_on_the_ground -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn search_cost_on_the_ground() {
        let wolf = path::Walker { height: 1, climbs: true, wary: true, span: 1 };
        let bear = path::Walker { height: 2, span: 2, ..wolf };
        for door in [0, 1, 2] {
            let mut server = crate::test_server();
            let c = pen(&mut server, 30);
            super::cornered_tests::wall(&mut server, (-29..=29).filter(|z| !(0..door).contains(z)).map(|z| c + IVec3::new(0, 0, z)), 3);
            for (name, walker) in [("wolf", wolf), ("bear", bear)] {
                let (from, to) = (c + IVec3::new(-4, 0, 0), c + IVec3::new(4, 0, 0));
                let counted = Counted(Ground::of(&server), std::cell::Cell::new(0));
                let found = path::find(&counted, &walker, from, to).is_some();
                let runs = 50;
                let at = Instant::now();
                for _ in 0..runs {
                    std::hint::black_box(path::find(&Ground::of(&server), &walker, from, to));
                }
                let ms = at.elapsed().as_secs_f64() * 1000.0 / runs as f64;
                println!("{name} door {door}: found {found}, {} lookups, {ms:.3} ms", counted.1.get());
            }
        }
    }
}

/// The gate's boss step: the Rootmother summoned on the creatures' pad (stone underfoot,
/// walled two high, a campfire in its middle), left to wander while the player is in
/// creative, then the player put a block before the pad's middle in survival. She must wind
/// up a told move within the step's 10 server seconds.
#[cfg(test)]
mod boss_tests {
    use super::*;

    /// The pad, high over spawn in empty columns; returns its middle at feet level.
    fn gate_pad(server: &mut Server) -> IVec3 {
        let pad = server.spawn.floor().as_ivec3() + IVec3::Y * 40;
        let (from, to) = (ColumnPos::of_block(pad - IVec3::splat(8)), ColumnPos::of_block(pad + IVec3::splat(8)));
        for z in from.0.y..=to.0.y {
            for x in from.0.x..=to.0.x {
                server.world.insert(ColumnPos::new(x, z), std::sync::Arc::new(voxmar_engine::world::Column::default()));
            }
        }
        let stone = server.content.blocks.expect("base:stone");
        for z in -6i32..=6 {
            for x in -6i32..=6 {
                for y in -1..=1 {
                    if y == -1 || x.abs() == 6 || z.abs() == 6 {
                        server.world.set_block(pad + IVec3::new(x, y, z), stone);
                    }
                }
            }
        }
        server.world.set_block(pad, server.content.blocks.expect("base:campfire"));
        pad
    }

    /// Seconds until she winds up a told move at the player, if she does within 10; after
    /// `wander` seconds about the pad with the player in creative. `run` sets the tick
    /// (and so her seed).
    fn told(run: u64, wander: u64) -> Option<f32> {
        let mut server = crate::test_server();
        server.tick = 1000 + run * 7919;
        let pad = gate_pad(&mut server);
        let (_client, connection) = voxmar_protocol::local_pair();
        let mut player = crate::player::Player::new(1, connection, pad.as_vec3() + Vec3::new(0.5, 0.0, 4.5), 2, server.tick, server.survival.max_health, &server.config.limits);
        (player.joined, player.allow_flight) = (true, true);
        server.players.insert(1, player);
        let species = server.content.creatures.find("base:rootmother").expect("the Rootmother");
        // Where `/summon rootmother 1` at the pad's front puts her.
        let id = server.spawn_creature(species, pad.as_vec3() + Vec3::new(-1.3, 0.0, 1.5), server.tick).expect("room");
        for _ in 0..wander * TICKS_PER_SECOND as u64 {
            server.tick += 1;
            server.tick_creatures();
        }
        let player = server.players.get_mut(&1).unwrap();
        (player.allow_flight, player.position) = (false, pad.as_vec3() + Vec3::new(0.5, 0.0, -1.0));
        (0..10 * TICKS_PER_SECOND as u64)
            .find(|_| {
                server.tick += 1;
                server.tick_creatures();
                server.players.get_mut(&1).unwrap().health = traits::Health::full(server.survival.max_health);
                let tell = server.creatures.live[&id].tell();
                tell != 0 && tell != voxmar_protocol::REARING && tell != voxmar_protocol::STAGGERED
            })
            .map(|t| t as f32 / TICKS_PER_SECOND as f32)
    }

    /// Runs that failed. Before: on main, 12 runs of 48 left her by the pad's wall,
    /// hopping at it with her head to it (she could not turn without sweeping it into the
    /// wall), never closing on the player; as the gate saw one run in six ("Idle, moving
    /// 0.0", standing 0.2 to 0.6 blocks up). With the movement fixes, one of 48 had her
    /// pressed against the player, inside her moves' reach from her edge, where she struck
    /// with a plain blow that has no tell.
    #[test]
    fn the_rootmother_on_the_gate_pad_always_winds_up_a_told_move() {
        for (run, wander) in [(1, 9), (7, 15), (11, 19), (13, 8), (41, 10), (0, 8)] {
            assert!(told(run, wander).is_some(), "run {run}: no told move of hers wound up in 10 s");
        }
    }
}

/// Fights at walls and doorways: a player stands by a wall's end, in a doorway, behind a
/// pillar or in a corner and strikes a creature each second it is in their reach, as the
/// owner played it. Measures how soon and how often it strikes back, the seconds it spends
/// pinned (wanting to go, not getting anywhere, out of its reach of the player) and the
/// hops it makes at a wall.
#[cfg(test)]
mod fight_tests {
    use super::cornered_tests::{join, pen, put, wall};
    use super::*;

    pub(super) const FIGHTERS: [&str; 6] = ["base:wolf", "base:brown_bear", "base:badger", "base:aurochs", "base:bison", "base:wild_boar"];
    /// The fighters wider than a block, which take no way through a doorway a block across.
    pub(super) const WIDE: [&str; 3] = ["base:brown_bear", "base:aurochs", "base:bison"];
    pub(super) const PREY: [&str; 4] = ["base:red_deer", "base:mouflon", "base:rabbit", "base:junglefowl"];

    #[derive(Debug, Default)]
    pub(super) struct Fight {
        pub first: Option<f32>,
        pub strikes: u32,
        pub pinned: f32,
        pub pressing: f32,
        pub cornered: f32,
        pub hops: u32,
        pub nearest: f32,
        /// How far it ended from the player.
        pub last: f32,
        /// The highest its feet came above the pen's floor, and the farthest it came east
        /// of the pen's centre (towards the player in the ground scenes).
        pub highest: f32,
        pub farthest: f32,
    }

    /// A scene: walls set about the pen's centre `c`, and where the player stands and the
    /// creature starts.
    pub(super) type Scene = fn(&mut Server, IVec3) -> (Vec3, Vec3);

    pub(super) fn scenes() -> [(&'static str, Scene); 8] {
        [
            ("wall back", |_, c| {
                // The player with their back to the pen's wall.
                (c.as_vec3() + Vec3::new(-7.7, 0.0, 0.5), c.as_vec3() + Vec3::new(-3.5, 0.0, 0.5))
            }),
            ("doorway", |s, c| {
                // A wall across at x = 1, a gap at z = 3; the player stands in the gap.
                wall(s, (-13..=13).filter(|&z| z != 3).map(|z| c + IVec3::new(1, 0, z)), 3);
                (c.as_vec3() + Vec3::new(1.5, 0.0, 3.5), c.as_vec3() + Vec3::new(-4.5, 0.0, 0.5))
            }),
            ("past door", |s, c| {
                wall(s, (-13..=13).filter(|&z| z != 3).map(|z| c + IVec3::new(1, 0, z)), 3);
                (c.as_vec3() + Vec3::new(3.0, 0.0, 3.5), c.as_vec3() + Vec3::new(-4.5, 0.0, 0.5))
            }),
            ("past wide door", |s, c| {
                // The doorway two across (z = 3 and 4): wide enough for a bear.
                wall(s, (-13..=13).filter(|z| !(3..=4).contains(z)).map(|z| c + IVec3::new(1, 0, z)), 3);
                (c.as_vec3() + Vec3::new(3.0, 0.0, 4.0), c.as_vec3() + Vec3::new(-4.5, 0.0, 0.5))
            }),
            ("staggered", |s, c| {
                // A wall two thick, its doorways two across a block apart (z = 2, 3 through
                // the near side, 3, 4 through the far): a way a block across, none wider.
                wall(s, (-13..=13).filter(|z| !(2..=3).contains(z)).map(|z| c + IVec3::new(1, 0, z)), 3);
                wall(s, (-13..=13).filter(|z| !(3..=4).contains(z)).map(|z| c + IVec3::new(2, 0, z)), 3);
                (c.as_vec3() + Vec3::new(4.0, 0.0, 3.5), c.as_vec3() + Vec3::new(-4.5, 0.0, 0.5))
            }),
            ("wall end", |s, c| {
                // A wall along x = 1 from the pen's edge to z = 2; the player just round its end.
                wall(s, (-13..=2).map(|z| c + IVec3::new(1, 0, z)), 3);
                (c.as_vec3() + Vec3::new(2.8, 0.0, 1.5), c.as_vec3() + Vec3::new(-2.5, 0.0, 0.5))
            }),
            ("pillar", |s, c| {
                wall(s, [c], 3);
                (c.as_vec3() + Vec3::new(2.0, 0.0, 0.5), c.as_vec3() + Vec3::new(-2.0, 0.0, 0.5))
            }),
            ("corner", |_, c| {
                // The player with their back in the pen's corner.
                (c.as_vec3() + Vec3::new(-7.7, 0.0, -7.7), c.as_vec3() + Vec3::new(-3.5, 0.0, -3.5))
            }),
        ]
    }

    /// Stone (or `block`) on each of `cells`, by name.
    fn set(s: &mut Server, block: &str, cells: impl IntoIterator<Item = IVec3>) {
        let id = s.content.blocks.expect(block);
        for cell in cells {
            s.world.set_block(cell, id);
        }
    }

    /// Uneven ground in a fight: a step, steps, a slab and block, logs lying about, tall
    /// grass, and a fence (two blocks high: the game has no fence block yet) or a ledge the
    /// player stands behind or on, out of the creature's way.
    pub(super) fn ground_scenes() -> [(&'static str, Scene); 7] {
        [
            ("step", |s, c| {
                set(s, "base:stone", (1..=4).flat_map(|x| (-3..=3).map(move |z| c + IVec3::new(x, 0, z))));
                (c.as_vec3() + Vec3::new(2.5, 1.0, 0.5), c.as_vec3() + Vec3::new(-4.5, 0.0, 0.5))
            }),
            ("steps", |s, c| {
                for x in 1..=3 {
                    set(s, "base:stone", (0..x).flat_map(|y| (-2..=2).map(move |z| c + IVec3::new(x, y, z))));
                }
                (c.as_vec3() + Vec3::new(3.5, 3.0, 0.5), c.as_vec3() + Vec3::new(-4.5, 0.0, 0.5))
            }),
            ("slab", |s, c| {
                set(s, "base:oak_slab", (-2..=2).map(|z| c + IVec3::new(1, 0, z)));
                set(s, "base:oak_planks", (2..=4).flat_map(|x| (-2..=2).map(move |z| c + IVec3::new(x, 0, z))));
                (c.as_vec3() + Vec3::new(3.0, 1.0, 0.5), c.as_vec3() + Vec3::new(-4.5, 0.0, 0.5))
            }),
            ("logs", |s, c| {
                set(s, "base:oak_log", [(-2, 0), (-1, 1), (0, -1), (-1, -1), (0, 1), (1, 0), (-3, 2), (-2, -2)].map(|(x, z)| c + IVec3::new(x, 0, z)));
                (c.as_vec3() + Vec3::new(2.5, 0.0, 0.5), c.as_vec3() + Vec3::new(-4.5, 0.0, 0.5))
            }),
            ("tall grass", |s, c| {
                let area = |y: i32| (-4..=1).flat_map(move |x| (-3..=3).map(move |z| c + IVec3::new(x, y, z)));
                set(s, "base:tall_grass", area(0));
                set(s, "base:tall_grass_top", area(1));
                (c.as_vec3() + Vec3::new(2.5, 0.0, 0.5), c.as_vec3() + Vec3::new(-4.5, 0.0, 0.5))
            }),
            ("fence", |s, c| {
                wall(s, (-8..=8).map(|z| c + IVec3::new(1, 0, z)), 2);
                (c.as_vec3() + Vec3::new(2.5, 0.0, 0.5), c.as_vec3() + Vec3::new(-4.5, 0.0, 0.5))
            }),
            ("ledge", |s, c| {
                set(s, "base:stone", (1..=8).flat_map(|x| (-8..=8).flat_map(move |z| (0..2).map(move |y| c + IVec3::new(x, y, z)))));
                (c.as_vec3() + Vec3::new(2.0, 2.0, 0.5), c.as_vec3() + Vec3::new(-4.5, 0.0, 0.5))
            }),
        ]
    }

    /// `name` started at the scene's spot, struck once to anger it, then `seconds` of the
    /// player standing there striking it each second it is within 3.5 blocks.
    pub(super) fn fight(name: &str, scene: Scene, seconds: f32) -> Fight {
        fight_with(name, scene, seconds, |_, _| {})
    }

    /// As `fight`, with `prep` done to the creature once it is put in.
    pub(super) fn fight_with(name: &str, scene: Scene, seconds: f32, prep: fn(&mut Server, u32)) -> Fight {
        let mut server = crate::test_server();
        let c = pen(&mut server, 9);
        let (stand, from) = scene(&mut server, c);
        let player = join(&mut server, stand);
        let id = put(&mut server, name, from, stand - from);
        prep(&mut server, id);
        server.tick = 1000;
        server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
        let mut fight = Fight { nearest: f32::MAX, highest: f32::MIN, farthest: f32::MIN, ..Default::default() };
        let dt = 1.0 / TICKS_PER_SECOND as f32;
        let (mut was_attacking, mut rising, mut still_for, mut last) = (false, false, 0.0f32, server.creatures.live[&id].body.position);
        for t in 0..(seconds * TICKS_PER_SECOND as f32) as u64 {
            server.tick += 1;
            let d = server.players[&player].position.distance(server.creatures.live[&id].body.position);
            if t % TICKS_PER_SECOND as u64 == 0 && d < 3.5 {
                server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
            }
            server.think_and_move();
            server.players.get_mut(&player).unwrap().health = traits::Health::full(server.survival.max_health);
            let creature = &server.creatures.live[&id];
            let body = creature.body;
            let attacking = creature.decision.attack.is_some();
            if attacking && !was_attacking {
                fight.strikes += 1;
                fight.first.get_or_insert(t as f32 * dt);
            }
            was_attacking = attacking;
            let up = body.velocity.y > 2.0;
            if up && !rising {
                fight.hops += 1;
            }
            rising = up;
            if std::env::var("VOXMAR_TRACE").is_ok_and(|v| v == name) && t % 5 == 0 {
                println!("t {t:>4} at {:.2} goal {:?} speed {:.2} doing {} stuck {:.2} detour {} way {} wall {}", body.position - c.as_vec3(), creature.decision.goal.map(|g| g - c.as_vec3()), creature.decision.speed, creature.mind.doing.map_or("-", |i| server.content.creatures.get(creature.species).modules[i].0.name()), creature.stuck, creature.detour, creature.way.len(), body.against_wall);
            }
            let flat = body.position.with_y(0.0).distance(stand.with_y(0.0));
            fight.nearest = fight.nearest.min(flat);
            fight.highest = fight.highest.max(body.position.y - c.y as f32);
            fight.farthest = fight.farthest.max(body.position.x - c.x as f32);
            // Wanting to go (a goal, not winding up or striking), out of reach, and no more
            // than a tenth of a block from where it stood a second ago.
            let wants = creature.decision.goal.is_some() && !attacking && server.tick >= creature.held_until;
            // In the player's reach, pressing a wall or going round one, or standing.
            if d < 3.5 && !attacking && (body.against_wall || creature.detour != 0.0 || body.velocity.with_y(0.0).length() < 0.2) {
                fight.cornered += dt;
            }
            // Going at the player and pressing a wall or going round one, out of its reach.
            if wants && (body.against_wall || creature.detour != 0.0) && creature.decision.goal.is_some_and(|g| g.distance(stand) < 0.5) {
                fight.pressing += dt;
            }
            if wants && body.position.distance(last) < 0.1 * (still_for + dt).min(1.0) {
                still_for += dt;
                if still_for >= 1.0 {
                    fight.pinned += dt;
                }
            } else {
                still_for = 0.0;
                last = body.position;
            }
        }
        fight.last = server.creatures.live[&id].body.position.with_y(0.0).distance(stand.with_y(0.0));
        fight
    }

    /// Every fighter, in every scene it fits, strikes back within 3 s and presses a wall for
    /// under 2 s of the 20. Too wide for the way to the player (a bear, an aurochs, a bison,
    /// with the player past a doorway a block across or a staggered wall), it presses for
    /// under 4 s, gives up, and struck again runs (`Memory::NoWay`). Before: those pressed
    /// the wall beside the doorway 19 s of the 20 under the player's blows, their way round
    /// planned through a gap their bodies could not pass; and a bison jostled in the near
    /// doorway of the staggered wall pressed it 17.7 s, each tick's headway clearing its
    /// stuck clock.
    #[test]
    fn fighters_at_walls_and_doorways_strike_back_or_give_up_rather_than_press() {
        // A scene to a thread: each fight is its own server.
        std::thread::scope(|threads| {
            for (scene, set) in scenes() {
                threads.spawn(move || {
                    for name in FIGHTERS {
                        let f = fight(name, set, 20.0);
                        if matches!(scene, "past door" | "staggered") && WIDE.contains(&name) {
                            assert!(f.pressing < 4.0, "{scene}: {name} pressed the wall {:.1} s", f.pressing);
                            assert!(f.strikes == 0, "{scene}: {name} struck through a way it cannot pass");
                            assert!(f.last > 3.5, "{scene}: {name} ended {:.2} blocks from the player, in their reach", f.last);
                        } else {
                            assert!(f.first.is_some_and(|s| s < 3.0), "{scene}: {name} first struck at {:?}", f.first);
                            assert!(f.pressing < 2.0, "{scene}: {name} pressed the wall {:.1} s", f.pressing);
                        }
                    }
                });
            }
        });
    }

    /// On uneven ground (a step, a flight of steps, a slab, logs lying about, tall grass)
    /// every fighter gets up to the player and strikes within 3 s, pressing a wall under
    /// 1 s of the 10. Behind a fence or up a ledge two blocks high, it never gets over (its
    /// feet never a block and a half up, its middle never in the first column), and strikes over it within
    /// 3.5 s or gives up, pressing under 4 s. Before: a wolf took 4.2 s to its first
    /// strike up three steps (pressing 2.0 s) and a badger 4.0 s, their heads meeting each
    /// step before their feet and turning them along it; and a wolf pressed the fence 16 s
    /// (given up since `Memory::NoWay`).
    #[test]
    fn fighters_on_uneven_ground_climb_to_strike_or_give_up() {
        // A scene to a thread: each fight is its own server.
        std::thread::scope(|threads| {
            for (scene, set) in ground_scenes() {
                threads.spawn(move || {
                    for name in FIGHTERS {
                        let f = fight(name, set, 10.0);
                        if matches!(scene, "fence" | "ledge") {
                            // Two blocks high: no creature hops it. Its feet never come up
                            // half way, nor its middle into the fence's or ledge's first
                            // column (x 1).
                            assert!(f.highest < 1.5 && f.farthest < 1.0, "{scene}: {name} got over: its feet {:.2} up, its middle {:.2} east", f.highest, f.farthest);
                            let gave_up = f.strikes == 0 && f.pressing < 4.0;
                            assert!(f.first.is_some_and(|s| s < 3.5) || gave_up, "{scene}: {name} first struck at {:?}, pressed {:.1} s", f.first, f.pressing);
                        } else {
                            assert!(f.first.is_some_and(|s| s < 3.0), "{scene}: {name} first struck at {:?}", f.first);
                            assert!(f.pressing < 1.0, "{scene}: {name} pressed a wall {:.1} s", f.pressing);
                        }
                    }
                });
            }
        });
    }

    /// Every long-headed jumper in the content (its head beyond its box), at scales 0.88,
    /// 1 and 1.12, driven straight up a flight of steps a block deep and three high, at a
    /// walk, a run and (Old Tusk) his charge: its head is never in a block, and at a run or
    /// faster, where a hop at that pace carries it its head's reach and a little, it gets to
    /// the top: Old Tusk at a run and a charge at every scale; only a full grown crocodile,
    /// whose low head will not fit on a step a block deep, is excused. Before: Old Tusk's
    /// snout met the second riser mid-hop and the head stop held him at the first; a hound's
    /// snout spent ticks in each riser.
    #[test]
    fn long_heads_climb_a_flight_of_steps_with_their_heads_out() {
        let mut server = crate::test_server();
        let c = pen(&mut server, 9);
        for x in 1..=3 {
            set(&mut server, "base:stone", (0..x).flat_map(|y| (-8..=8).map(move |z| c + IVec3::new(x, y, z))));
        }
        set(&mut server, "base:stone", (4..=8).flat_map(|x| (0..3).flat_map(move |y| (-8..=8).map(move |z| c + IVec3::new(x, y, z)))));
        let ground = Ground::of(&server);
        let (tusk, crocodile) = (server.content.creatures.find("old_tusk"), server.content.creatures.find("crocodile"));
        let mut tried = 0;
        for (id, species) in server.content.creatures.iter() {
            let movement = species.def.movement;
            let size = species.def.body.size;
            if movement.jump <= 0.0 || species.reach <= size.0 * 0.5 + 0.05 || species.def.movement.style == voxmar_engine::creature::Style::Flier {
                continue;
            }
            let charge = if Some(id) == tusk { Some(9.0) } else { None };
            for scale in [0.88, 1.0, 1.12] {
                for (pace, speed) in [("walk", movement.walk), ("run", movement.run)].into_iter().chain(charge.map(|c| ("charge", c))) {
                    let mut body = Body::new(c.as_vec3() + Vec3::new(-2.5, 0.0, 0.5), motion::yaw_of(Vec3::X), (size.0 * scale, size.1 * scale));
                    (body.reach, body.head, body.on_ground) = (species.reach * scale, species.head * scale, true);
                    // Until it is on the top: the six blocks to it at its pace, and five seconds.
                    for t in 0..((6.0 / speed + 5.0) * TICKS_PER_SECOND as f32) as u32 {
                        motion::step(&mut body, &ground, &movement, Water::Land, Drive { wish: Vec3::X, speed, face: None }, 1.0 / TICKS_PER_SECOND as f32);
                        assert!(!body.head_blocked(&ground), "{} at {scale} ({pace}): its head in a step {t} ticks on, at {:.2}", species.def.name, body.position - c.as_vec3());
                        if body.position.y >= c.y as f32 + 3.0 {
                            break;
                        }
                    }
                    // A hop at its pace, as `step` hops a step, carries it its head's reach and
                    // a little (`head_step`).
                    let hops_far = speed * 2.0 * motion::hop_speed(&movement, 1.0 / TICKS_PER_SECOND as f32) / motion::GRAVITY >= body.reach - body.size.0 * 0.5 + 0.3;
                    // Standing on a step a block deep with room to land (a fifth of a block),
                    // its head is short of the next riser, or carried over it: not a full
                    // grown crocodile's, low and nine tenths of a block out from its box. Only
                    // the crocodile is excused that way: Old Tusk and a small wild horse get
                    // up with their heads out all the same.
                    let fits = body.reach - body.size.0 * 0.5 < 0.8 || body.head * 0.7 >= 1.0;
                    let excused = !fits && Some(id) == crocodile;
                    if pace != "walk" && hops_far && !excused {
                        assert!(body.position.y >= c.y as f32 + 3.0, "{} at {scale} ({pace}) never got up the flight: at {:.2}", species.def.name, body.position - c.as_vec3());
                    }
                    assert!(Some(id) != tusk || pace == "walk" || hops_far, "Old Tusk at {scale} ({pace}) is excused the flight as hopping short");
                    tried += 1;
                }
            }
        }
        assert!(tried > 0 && tusk.is_some() && crocodile.is_some(), "no long-headed jumper, or no Old Tusk or crocodile, in the content");
    }

    /// A fighter as wide as a wild boar (its body from data, at the creature's scale), with
    /// the player past a doorway a block across, lines up with the doorway and goes through:
    /// it strikes within 3 s and presses the wall under 2 s. Before: a boar (0.8 wide) cut
    /// the corner for the doorway's far cell, met the frame, and dithered there, its way
    /// round flipping side to side (z swinging 2.4 to 4.6 about the doorway's 3.5) for 5 s;
    /// it first struck at 6.95 s.
    #[test]
    fn a_body_as_wide_as_a_boar_lines_up_with_a_doorway_and_goes_through() {
        let past = scenes().into_iter().find(|(name, _)| *name == "past door").expect("the scene").1;
        let boar: fn(&mut Server, u32) = |s, id| {
            let size = s.content.creatures.get(s.content.creatures.find("wild_boar").expect("a wild boar")).def.body.size;
            let live = s.creatures.live.get_mut(&id).unwrap();
            live.body.size = (size.0 * live.scale, size.1 * live.scale);
        };
        let width = {
            let s = crate::test_server();
            let id = s.content.creatures.find("wild_boar").expect("a wild boar");
            s.content.creatures.get(id).def.body.size.0
        };
        assert!(width > 0.7 && width < 1.0, "the fixture means a body between a wolf's width and a block: {width}");
        for name in ["base:wolf", "base:badger"] {
            let fight = fight_with(name, past, 20.0, boar);
            assert!(fight.first.is_some_and(|t| t <= 3.0) && fight.pressing < 2.0, "{name} as wide as a boar past a doorway: {fight:?}");
        }
    }

    /// A trench a block wide and a block deep across the pen at x = 1.
    fn trench(s: &mut Server, c: IVec3) {
        set(s, "base:stone", (-8..=8).map(|z| c + IVec3::new(1, -2, z)));
        for z in -8..=8 {
            s.world.set_block(c + IVec3::new(1, -1, z), BlockId::AIR);
        }
    }

    /// Every jumper a block wide or less driven straight across a trench a block wide and
    /// deep at a run, at scales 0.88 to 1.12, never hangs in the air (no two ticks with
    /// nothing under its box and not moving) and, where a hop carries it out, gets over. At a
    /// walk a long head may still be held over the trench's far side for a few ticks, until
    /// its brain gives the way up. Before: a long head meeting the trench's far side as it
    /// came down was put back where it was, still off the ground, and fell into the same
    /// place each tick (145 to 190 ticks of 200 for long-headed jumpers).
    #[test]
    fn jumpers_cross_a_trench_without_hanging_in_the_air() {
        let mut server = crate::test_server();
        let c = pen(&mut server, 9);
        trench(&mut server, c);
        let ground = Ground::of(&server);
        let mut tried = 0;
        for (_, species) in server.content.creatures.iter() {
            let movement = species.def.movement;
            let size = species.def.body.size;
            if movement.jump <= 0.0 || species.def.movement.style == voxmar_engine::creature::Style::Flier || size.0 > 1.0 {
                continue;
            }
            for scale in [0.88, 1.0, 1.12] {
                for (pace, speed) in [("walk", movement.walk), ("run", movement.run)] {
                    let mut body = Body::new(c.as_vec3() + Vec3::new(-1.5, 0.0, 0.5), motion::yaw_of(Vec3::X), (size.0 * scale, size.1 * scale));
                    (body.reach, body.head, body.on_ground) = (species.reach * scale, species.head * scale, true);
                    let (mut hung, mut was) = (0, body.position);
                    // Nothing under its box: it should be falling.
                    let unsupported = |body: &Body| {
                        let mut under = body.aabb();
                        under.translate(Vec3::Y * -0.05);
                        !under.intersects(|p| motion::Surroundings::solid(&ground, p))
                    };
                    for t in 0..((5.0 / speed + 5.0) * TICKS_PER_SECOND as f32) as u32 {
                        motion::step(&mut body, &ground, &movement, Water::Land, Drive { wish: Vec3::X, speed, face: None }, 1.0 / TICKS_PER_SECOND as f32);
                        hung = if unsupported(&body) && body.position == was { hung + 1 } else { 0 };
                        assert!(pace == "walk" || hung < 2, "{} at {scale} ({pace}) hangs in the air {t} ticks on, at {:.2}", species.def.name, body.position - c.as_vec3());
                        was = body.position;
                        if body.position.x - c.x as f32 > 3.0 {
                            break;
                        }
                    }
                    // At a run, where a hop carries its box over the trench's far edge, it
                    // gets out (`head_step`'s reach and a little).
                    let hops_far = speed * 2.0 * motion::hop_speed(&movement, 1.0 / TICKS_PER_SECOND as f32) / motion::GRAVITY >= body.reach.max(body.size.0 * 0.5) - body.size.0 * 0.5 + 0.3;
                    if pace != "walk" && hops_far {
                        assert!(body.position.x - c.x as f32 > 3.0, "{} at {scale} ({pace}) never got over the trench: at {:.2}", species.def.name, body.position - c.as_vec3());
                    }
                    tried += 1;
                }
            }
        }
        assert!(tried > 0);
    }

    /// Every jumper a block wide or less steering (as its brain drives it) at a walk for a
    /// trench a block wide and deep never hangs in the air: where its head, landed in the
    /// trench, would meet the far side and it cannot hop out at a walk, it takes no way down
    /// into it (`open_way`). Before: a mouflon, a small red deer or a wild boar walked in and
    /// was held over the far side, its head stopping it, for up to 8 ticks at a time.
    #[test]
    fn walkers_steering_for_a_trench_never_hang_in_it() {
        let mut server = crate::test_server();
        let c = pen(&mut server, 9);
        trench(&mut server, c);
        let ground = Ground::of(&server);
        let dt = 1.0 / TICKS_PER_SECOND as f32;
        let (mut tried, mut crossed, mut refused) = (0, 0, 0);
        for (_, species) in server.content.creatures.iter() {
            let movement = species.def.movement;
            let size = species.def.body.size;
            if movement.jump <= 0.0 || species.def.movement.style == voxmar_engine::creature::Style::Flier || size.0 > 1.0 {
                continue;
            }
            for scale in [0.88, 1.0, 1.12] {
                let mut body = Body::new(c.as_vec3() + Vec3::new(-1.5, 0.0, 0.5), motion::yaw_of(Vec3::X), (size.0 * scale, size.1 * scale));
                (body.reach, body.head, body.on_ground) = (species.reach * scale, species.head * scale, true);
                // Whether, set down in the trench facing across where it fits with its head
                // clear, it steers out over the far side: then the way in is open to it too,
                // and it gets over.
                let half = body.size.0 * 0.5;
                let gets_out = (0..=20).map(|k| Body { position: c.as_vec3() + Vec3::new(1.0 + half + (1.0 - 2.0 * half) * k as f32 / 20.0, -1.0, 0.5), ..body }).find(|down| !down.aabb().intersects(|p| motion::Surroundings::solid(&ground, p)) && !down.head_blocked(&ground)).is_some_and(|mut down| {
                    let mut side = 0.0;
                    for _ in 0..3 * TICKS_PER_SECOND {
                        let (heading, turned) = motion::steer(&down, &ground, &movement, Water::Land, Vec3::X, side, movement.walk, dt);
                        side = turned;
                        motion::step(&mut down, &ground, &movement, Water::Land, Drive { wish: heading, speed: movement.walk, face: None }, dt);
                    }
                    down.position.x - c.x as f32 > 2.0 && down.position.y > c.y as f32 - 0.5
                });
                let (mut hung, mut was, mut side, mut held_down) = (0, body.position, 0.0, 0);
                for t in 0..(10 * TICKS_PER_SECOND) {
                    let (heading, turned) = motion::steer(&body, &ground, &movement, Water::Land, Vec3::X, side, movement.walk, dt);
                    side = turned;
                    motion::step(&mut body, &ground, &movement, Water::Land, Drive { wish: heading, speed: movement.walk, face: None }, dt);
                    let mut under = body.aabb();
                    under.translate(Vec3::Y * -0.05);
                    hung = if !under.intersects(|p| motion::Surroundings::solid(&ground, p)) && body.position == was { hung + 1 } else { 0 };
                    assert!(hung < 2, "{} at {scale} hangs in the air {t} ticks on, at {:.2}", species.def.name, body.position - c.as_vec3());
                    was = body.position;
                    held_down = if body.position.y < c.y as f32 - 0.5 { held_down + 1 } else { 0 };
                    if body.position.x - c.x as f32 > 3.0 {
                        break;
                    }
                }
                // Never trapped down in it (a long head that got in, unable to go on, climbs
                // back out the way it came), and over it where it can get out ahead.
                let over = body.position.x - c.x as f32 > 3.0;
                assert!(held_down < 3 * TICKS_PER_SECOND, "{} at {scale} is trapped in the trench: at {:.2} facing {:.2}", species.def.name, body.position - c.as_vec3(), body.forward());
                assert!(over || !gets_out, "{} at {scale} never got over the trench, though it steers out of it: at {:.2} facing {:.2}", species.def.name, body.position - c.as_vec3(), body.forward());
                (crossed, refused) = (crossed + over as u32, refused + !over as u32);
                tried += 1;
            }
        }
        // Both kinds are tried: some get over, some long heads keep out.
        assert!(tried > 0 && crossed > 0 && refused > 0, "{crossed} of {tried} got over, {refused} kept out");
    }

    /// Going straight to a cell is the straight line: a box a block diagonally from its next
    /// cell, a block on the corner between, does not go straight there. Before: swept an axis
    /// at a time (along x, then z) it went round the corner and counted as straight, and a
    /// walker cut the corner of a stair's turn into the block.
    #[test]
    fn a_way_round_a_corner_is_not_straight() {
        let body = Body::new(Vec3::new(0.5, 0.0, 0.5), 0.0, (0.6, 1.0));
        let corner = |p: IVec3| p == IVec3::new(0, 0, 1);
        assert!(!goes_straight(body.aabb(), Vec3::new(1.0, 0.0, 1.0), corner));
        assert!(goes_straight(body.aabb(), Vec3::new(1.0, 0.0, 0.0), corner));
        assert!(goes_straight(body.aabb(), Vec3::new(1.0, 0.0, 1.0), |_| false));
    }

    /// Every land kind walking (grazing, herding, an errand) for open floor below a ledge 1,
    /// 2 or 3 blocks high goes down onto it: open ground below is a way on, however far it
    /// walks there. Before (375's first cut): a way down where walking eight ways met nothing
    /// to climb out by was refused at a walk, and long heads stayed up at a 1-block ledge,
    /// and nearly every kind (rabbits, wolves, every villager) at 2 or 3.
    #[test]
    fn walkers_go_down_a_ledge_onto_open_ground() {
        let dt = 1.0 / TICKS_PER_SECOND as f32;
        for high in 1..=3 {
            let mut server = crate::test_server();
            let c = pen(&mut server, 9);
            // A plateau `high` up west of x = 1, the pen's walls raised over it.
            set(&mut server, "base:stone", (-8..=0).flat_map(|x| (-8..=8).flat_map(move |z| (0..high).map(move |y| c + IVec3::new(x, y, z)))));
            let rim = (-9..=9).flat_map(|i| [IVec3::new(i, high, -9), IVec3::new(i, high, 9), IVec3::new(-9, high, i), IVec3::new(9, high, i)]);
            wall(&mut server, rim.map(|cell| c + cell).collect::<Vec<_>>(), 3);
            let ground = Ground::of(&server);
            let mut tried = 0;
            for (_, species) in server.content.creatures.iter() {
                let movement = species.def.movement;
                if species.def.habitat.water != Water::Land || matches!(movement.style, voxmar_engine::creature::Style::Flier | voxmar_engine::creature::Style::Drifter) {
                    continue;
                }
                let size = species.def.body.size;
                let mut body = Body::new(c.as_vec3() + Vec3::new(-3.5, high as f32, 0.5), motion::yaw_of(Vec3::X), size);
                (body.reach, body.head, body.on_ground) = (species.reach, species.head, true);
                // Long enough to walk the 4.5 blocks to the lip and off it, slow as a snail.
                let (mut side, ticks) = (0.0, ((4.5 / movement.walk + 4.0) * TICKS_PER_SECOND as f32) as u32);
                for _ in 0..ticks {
                    let (heading, turned) = motion::steer(&body, &ground, &movement, Water::Land, Vec3::X, side, movement.walk, dt);
                    side = turned;
                    motion::step(&mut body, &ground, &movement, Water::Land, Drive { wish: heading, speed: movement.walk, face: None }, dt);
                    if body.on_ground && body.position.y < c.y as f32 + 0.5 && body.position.x > c.x as f32 + 1.0 {
                        break;
                    }
                }
                assert!(body.position.y < c.y as f32 + 0.5, "{} stays up a ledge {high} high at a walk: at {:.2}", species.def.name, body.position - c.as_vec3());
                tried += 1;
            }
            assert!(tried > 0);
        }
    }

    /// A pit `n` across and three deep from x = 2, its walls and floor stone, the player in
    /// its middle; struck from the rim's side.
    fn pit(s: &mut Server, c: IVec3, n: i32) -> (Vec3, Vec3) {
        set(s, "base:stone", (1..=n + 2).flat_map(|x| (-1..=n).flat_map(move |z| (-4..=-2).map(move |y| c + IVec3::new(x, y, z)))));
        for (x, z, y) in (2..=n + 1).flat_map(|x| (0..n).flat_map(move |z| (-3..=-1).map(move |y| (x, z, y)))) {
            s.world.set_block(c + IVec3::new(x, y, z), BlockId::AIR);
        }
        (c.as_vec3() + Vec3::new(2.0 + n as f32 / 2.0, -3.0, n as f32 / 2.0), c.as_vec3() + Vec3::new(-2.5, 0.0, n as f32 / 2.0))
    }

    /// A stair a block wide cut down into stone along x, three treads to a landing two
    /// across a step lower, then on along z a block wide to the player in a room two across
    /// at its foot, walled from the ground before it: an L going down, open over its treads
    /// and roofed past them by a wall two high so the way to the player is down it, not
    /// over their head.
    fn l_stair(s: &mut Server, c: IVec3) -> (Vec3, Vec3) {
        set(s, "base:stone", (1..=7).flat_map(|x| (-1..=6).flat_map(move |z| (-6..=-1).map(move |y| c + IVec3::new(x, y, z)))));
        set(s, "base:stone", (4..=7).flat_map(|x| (-1..=6).flat_map(move |z| (0..=1).map(move |y| c + IVec3::new(x, y, z)))));
        // Walled up to its head, a block wide, from the ground before it.
        wall(s, (-3..=1).flat_map(|x| [c + IVec3::new(x, 0, -1), c + IVec3::new(x, 0, 1)]), 2);
        let treads = [(1, -1, 0), (2, -2, 0), (3, -3, 0), (4, -4, 0), (5, -4, 0), (4, -4, 1), (5, -4, 1), (5, -4, 2), (5, -4, 3), (6, -4, 3), (5, -4, 4), (6, -4, 4)];
        for (x, feet, z) in treads {
            for y in feet..=feet + 3 {
                s.world.set_block(c + IVec3::new(x, y, z), BlockId::AIR);
            }
        }
        (c.as_vec3() + Vec3::new(6.0, -4.0, 4.0), c.as_vec3() + Vec3::new(-0.5, 0.0, 0.5))
    }

    /// Sets a creature's scale, its body with it.
    fn rescale(s: &mut Server, id: u32, scale: f32) {
        let creature = s.creatures.live.get_mut(&id).unwrap();
        let k = scale / creature.scale;
        creature.scale = scale;
        creature.body.size = (creature.body.size.0 * k, creature.body.size.1 * k);
    }

    /// A wolf, a badger and Old Tusk, struck by a player down in a pit two or three across
    /// and three deep, or at the foot of a stair a block wide that turns going down, at
    /// scales 0.88 to 1.12, get down to them and strike back within 3 s. A chase drops in;
    /// only a walk refuses a way down its head cannot land in. Before (#353's first cut):
    /// Old Tusk circled a pit's rim and never struck, a wolf at a two-across pit never
    /// struck, and on the stair they stopped at the last tread over the landing.
    #[test]
    fn chasers_get_down_into_a_pit_or_a_turning_stair_to_strike() {
        let scenes: [(&str, Scene); 3] = [("pit 2", |s, c| pit(s, c, 2)), ("pit 3", |s, c| pit(s, c, 3)), ("L-stair", l_stair)];
        let scaled: [(f32, fn(&mut Server, u32)); 3] = [
            (0.88, |s, id| rescale(s, id, 0.88)),
            (1.0, |s, id| rescale(s, id, 1.0)),
            (1.12, |s, id| rescale(s, id, 1.12)),
        ];
        for (scene, set) in scenes {
            // Old Tusk, wider than a block, takes no stair a block wide.
            for name in ["base:wolf", "base:badger", "base:old_tusk"].into_iter().filter(|&name| !(scene == "L-stair" && name == "base:old_tusk")) {
                for (scale, prep) in scaled {
                    let f = fight_with(name, set, 6.0, prep);
                    assert!(f.first.is_some_and(|s| s < 3.0), "{scene}: {name} at {scale} first struck at {:?}, ending {:.2} from the player", f.first, f.last);
                }
            }
        }
    }

    /// Every fighter a block wide or less, struck from across a trench a block wide and deep,
    /// crosses and strikes back within 3 s. Before: a struck wolf hung over the trench 90
    /// ticks and never struck.
    #[test]
    fn a_struck_fighter_crosses_a_trench_to_strike() {
        let scene: Scene = |s, c| {
            trench(s, c);
            (c.as_vec3() + Vec3::new(3.5, 0.0, 0.5), c.as_vec3() + Vec3::new(-2.5, 0.0, 0.5))
        };
        for name in FIGHTERS.into_iter().filter(|name| !WIDE.contains(name)) {
            let fight = fight(name, scene, 8.0);
            assert!(fight.first.is_some_and(|t| t <= 3.0), "{name} across a trench: {fight:?}");
        }
    }

    /// A block a block high, a gap a block or two wide, then a fence two high: no creature,
    /// walking, running or (Old Tusk) charging at it, gets onto the fence or over it. Before:
    /// a mouflon, a red deer or an aurochs took the block, the gap and the fence for a stair
    /// (its head meeting the fence's upper block, judged at the block's top) and leapt onto
    /// the fence in 194 runs of 1104; and a snow leopard (its jump 11), its hop off the block
    /// springing on at its pace, came down on the fence two cells on.
    #[test]
    fn a_block_across_a_gap_is_no_stair_onto_a_fence() {
        for gap in [1, 2] {
            block_gap_fence(gap);
        }
    }

    fn block_gap_fence(gap: i32) {
        let mut server = crate::test_server();
        let c = pen(&mut server, 9);
        set(&mut server, "base:stone", (-8..=8).map(|z| c + IVec3::new(1, 0, z)));
        let fence = 2 + gap;
        wall(&mut server, (-8..=8).map(|z| c + IVec3::new(fence, 0, z)), 2);
        let ground = Ground::of(&server);
        let tusk = server.content.creatures.find("old_tusk");
        let mut tried = 0;
        for (id, species) in server.content.creatures.iter() {
            let movement = species.def.movement;
            let size = species.def.body.size;
            if movement.jump <= 0.0 || species.def.movement.style == voxmar_engine::creature::Style::Flier {
                continue;
            }
            let charge = if Some(id) == tusk { Some(9.0) } else { None };
            for scale in [0.88, 1.0, 1.12] {
                for ((pace, speed), from) in [("walk", movement.walk), ("run", movement.run)].into_iter().chain(charge.map(|c| ("charge", c))).flat_map(|p| [(p, Vec3::new(1.5, 1.0, 0.5)), (p, Vec3::new(-1.5, 0.0, 0.5))]) {
                    // From on the block, or on the ground before it, facing the gap and the fence.
                    let mut body = Body::new(c.as_vec3() + from, motion::yaw_of(Vec3::X), (size.0 * scale, size.1 * scale));
                    (body.reach, body.head, body.on_ground) = (species.reach * scale, species.head * scale, true);
                    for _ in 0..(4 * TICKS_PER_SECOND) {
                        motion::step(&mut body, &ground, &movement, Water::Land, Drive { wish: Vec3::X, speed, face: None }, 1.0 / TICKS_PER_SECOND as f32);
                        let at = body.position - c.as_vec3();
                        assert!(!(at.y >= 1.9 && at.x > fence as f32 - 0.6) && at.x < fence as f32 + 0.5, "{} at {scale} ({pace}) got onto or over the fence {gap} past the block from {from}: at {at:.2}", species.def.name);
                    }
                    tried += 1;
                }
            }
        }
        assert!(tried > 0);
    }

    /// A balcony four blocks up, railed along its east edge: no creature that walks,
    /// walking or running at the railing, goes over it or off the balcony, nor stands on air
    /// over it, driven straight at it or steering as a brain does: with the railing alone;
    /// with a block-high row of stone, or of stairs, just inside it; or railed on its south
    /// edge too, driven into the corner. Steering from up on the railing, or on the wall
    /// over it, it never walks off the balcony's edge either. (A boss climbs a railing:
    /// `a_fighter_or_a_boss_gets_into_a_ring_of_railings`.) Before: the cell
    /// over a railing, a wall to a creature, was a floor too, so anything on a block beside
    /// the railing hopped onto that cell and walked on air a block over the railing's top
    /// (111 of 159 species and pace runs got over); and one already up there found no ground
    /// under it to judge a drop by, so took every way for open and walked off the balcony.
    #[test]
    fn a_railing_keeps_creatures_on_a_balcony() {
        for scene in ["railing", "stone step", "stair", "corner", "on the railing", "over the railing"] {
            railed_balcony(scene);
        }
    }

    fn railed_balcony(scene: &str) {
        let mut server = crate::test_server();
        let c = pen(&mut server, 9);
        // The balcony: stone four high from the pen's west wall to x 2, the railing on its
        // edge; beyond it the pen's floor, four down. Its ends are walled up, so steering
        // along the railing it goes nowhere else. Railed into a corner, it ends at z 2.
        let corner = scene == "corner";
        let far = if corner { 2 } else { 8 };
        for x in -8..=2 {
            wall(&mut server, (-8..=far).map(|z| c + IVec3::new(x, 0, z)), 4);
        }
        set(&mut server, "base:oak_railing_west", (-8..if corner { far } else { far + 1 }).map(|z| c + IVec3::new(2, 4, z)));
        wall(&mut server, (-8..=3).flat_map(|x| [c + IVec3::new(x, 0, -9), c + IVec3::new(x, 0, 9)]), 8);
        let wish = if corner {
            set(&mut server, "base:oak_railing_south", (-8..2).map(|x| c + IVec3::new(x, 4, 2)));
            set(&mut server, "base:oak_railing_corner", [c + IVec3::new(2, 4, 2)]);
            Vec3::new(1.0, 0.0, 1.0).normalize()
        } else {
            Vec3::X
        };
        match scene {
            "stone step" => set(&mut server, "base:stone", (-8..=8).map(|z| c + IVec3::new(1, 4, z))),
            "stair" => set(&mut server, "base:oak_stairs_east", (-8..=8).map(|z| c + IVec3::new(1, 4, z))),
            _ => {}
        }
        let ground = Ground::of(&server);
        let dt = 1.0 / TICKS_PER_SECOND as f32;
        let mut tried = 0;
        for (_, species) in server.content.creatures.iter() {
            let movement = species.def.movement;
            let size = species.def.body.size;
            if movement.jump <= 0.0 || species.def.movement.style == voxmar_engine::creature::Style::Flier || size.1 > 3.0 {
                continue;
            }
            for (pace, speed) in [("walk", movement.walk), ("run", movement.run)] {
                for steers in [false, true] {
                    // Up on the railing's top, or on the wall over it, it starts where a
                    // chase or a fall from above could leave it: steering, it still judges
                    // the drop beyond from there.
                    let from = match scene {
                        "corner" => Vec3::new(-1.5, 4.0, -1.5),
                        "on the railing" => Vec3::new(2.5, 5.0, 0.5),
                        "over the railing" => Vec3::new(2.5, 6.0, 0.5),
                        _ => Vec3::new(-1.5, 4.0, 0.5),
                    };
                    let on_railing = from.x > 2.0;
                    if on_railing && !steers {
                        continue;
                    }
                    let mut body = Body::new(c.as_vec3() + from, motion::yaw_of(wish), size);
                    (body.reach, body.head, body.on_ground) = (species.reach, species.head, true);
                    let mut side = 0.0;
                    for _ in 0..(4 * TICKS_PER_SECOND) {
                        let heading = if steers {
                            let (heading, taken) = motion::steer(&body, &ground, &movement, Water::Land, wish, side, speed, dt);
                            side = taken;
                            heading
                        } else {
                            wish
                        };
                        motion::step(&mut body, &ground, &movement, Water::Land, Drive { wish: heading, speed, face: None }, dt);
                        let at = body.position - c.as_vec3();
                        let how = if steers { "steering" } else { "driven" };
                        assert!(at.y >= 3.99 && at.x < if on_railing { 3.5 } else { 3.0 } && !(corner && at.z >= 3.0), "{} ({pace}, {how}, {scene}) got over the railing or off the balcony: at {at:.2}", species.def.name);
                        // On the step it stands at 5; on air over the railing, at 6.
                        assert!(on_railing || at.y < 5.9, "{} ({pace}, {how}, {scene}) stood on air over the railing: at {at:.2}", species.def.name);
                    }
                    tried += 1;
                }
            }
        }
        assert!(tried > 0);
    }

    /// A player in a ring of railings, five across, is no player out of reach: a fighter
    /// they strike, and Old Tusk, climb the railing as the block-high step it is and strike
    /// within 6 s. Before: a railing stood two high to every creature, so neither got in in
    /// 10 s, and a boss, which never balks, pressed on the ring while a spear outreached it.
    #[test]
    fn a_fighter_or_a_boss_gets_into_a_ring_of_railings() {
        let scene: Scene = |s, c| {
            let ring = (-2..=2).flat_map(|i| [IVec3::new(i, 0, -2), IVec3::new(i, 0, 2), IVec3::new(-2, 0, i), IVec3::new(2, 0, i)]);
            set(s, "base:oak_railing", ring.map(|cell| c + IVec3::new(3, 0, 0) + cell).collect::<Vec<_>>());
            (c.as_vec3() + Vec3::new(3.5, 0.0, 0.5), c.as_vec3() + Vec3::new(-4.5, 0.0, 0.5))
        };
        for name in ["base:wolf", "base:old_tusk"] {
            let fight = fight(name, scene, 8.0);
            assert!(fight.first.is_some_and(|t| t <= 6.0), "{name} at a ring of railings: {fight:?}");
        }
    }

    /// A ring of railings, five across, keeps in what it pens: a mouflon led by wheat
    /// toward a player outside it, a red deer that player struck, and a kept boar they
    /// struck (which turns to defend itself) all stay inside, on its floor, through 10 s of
    /// the server's own ticks. Only a boss or a wild fighter on a chase climbs a railing.
    #[test]
    fn a_ring_of_railings_pens_the_led_the_struck_and_the_kept() {
        let mut server = crate::test_server();
        let c = pen(&mut server, 9);
        let middle = c + IVec3::new(3, 0, 0);
        let ring = (-2..=2).flat_map(|i| [IVec3::new(i, 0, -2), IVec3::new(i, 0, 2), IVec3::new(-2, 0, i), IVec3::new(2, 0, i)]);
        set(&mut server, "base:oak_railing", ring.map(|cell| middle + cell).collect::<Vec<_>>());
        let player = join(&mut server, c.as_vec3() + Vec3::new(-2.5, 0.0, 0.5));
        let wheat = server.content.blocks.expect("base:wheat");
        let held = server.players.get_mut(&player).unwrap();
        held.inventory.add(wheat, 1);
        held.selected = (0..36).find(|&slot| held.inventory.get(slot).is_some_and(|s| s.item == wheat)).expect("the wheat is held");
        let at = |dx: f32, dz: f32| middle.as_vec3() + Vec3::new(0.5 + dx, 0.0, 0.5 + dz);
        let sheep = put(&mut server, "base:mouflon", at(0.0, 0.0), -Vec3::X);
        let deer = put(&mut server, "base:red_deer", at(1.0, 1.0), -Vec3::X);
        let boar = put(&mut server, "base:wild_boar", at(-1.0, -1.0), -Vec3::X);
        server.creatures.live.get_mut(&boar).unwrap().owner = Some("Bob".into());
        server.tick = 1000;
        for id in [deer, boar] {
            server.hurt_creature(player, id, 0, None, voxmar_engine::combat::Blow::Plain);
        }
        for t in 0..10 * TICKS_PER_SECOND as u64 {
            server.tick += 1;
            server.think_and_move();
            server.players.get_mut(&player).unwrap().health = traits::Health::full(server.survival.max_health);
            for (id, what) in [(sheep, "the mouflon after wheat"), (deer, "the struck deer"), (boar, "the struck kept boar")] {
                let off = server.creatures.live[&id].body.position - middle.as_vec3();
                let inside = (-1.0..2.0).contains(&off.x) && (-1.0..2.0).contains(&off.z) && off.y < 0.9;
                assert!(inside, "{what} got out of the ring of railings {t} ticks on: at {off:.2} from its middle cell");
            }
        }
    }

    /// A field fence pens as a railing does: a mouflon led by wheat stays in a ring of fences
    /// (corners at the corners, a shut gate on the west side) for 10 s; with the gate swung
    /// open it follows the wheat out through it.
    #[test]
    fn a_fenced_pen_holds_a_led_mouflon_until_its_gate_opens() {
        let mut server = crate::test_server();
        let c = pen(&mut server, 9);
        let middle = c + IVec3::new(3, 0, 0);
        let mut fences = Vec::new();
        for i in -1..=1 {
            fences.push((IVec3::new(i, 0, -2), "base:oak_fence"));
            fences.push((IVec3::new(i, 0, 2), "base:oak_fence"));
            fences.push((IVec3::new(2, 0, i), "base:oak_fence_east"));
            fences.push((IVec3::new(-2, 0, i), if i == 0 { "base:oak_fence_gate_east" } else { "base:oak_fence_east" }));
        }
        for (cell, corner) in [((-2, -2), "base:oak_fence_corner_south"), ((2, -2), "base:oak_fence_corner_west"), ((2, 2), "base:oak_fence_corner"), ((-2, 2), "base:oak_fence_corner_east")] {
            fences.push((IVec3::new(cell.0, 0, cell.1), corner));
        }
        for (cell, name) in fences {
            let id = server.content.blocks.expect(name);
            server.set_block(middle + cell, id);
        }
        let player = join(&mut server, middle.as_vec3() + Vec3::new(-6.5, 0.0, 0.5));
        let wheat = server.content.blocks.expect("base:wheat");
        let held = server.players.get_mut(&player).unwrap();
        held.inventory.add(wheat, 1);
        held.selected = (0..36).find(|&slot| held.inventory.get(slot).is_some_and(|s| s.item == wheat)).expect("the wheat is held");
        let sheep = put(&mut server, "base:mouflon", middle.as_vec3() + Vec3::new(1.5, 0.0, 0.5), -Vec3::X);
        server.tick = 1000;
        let off = |server: &Server| server.creatures.live[&sheep].body.position - middle.as_vec3();
        for t in 0..10 * TICKS_PER_SECOND as u64 {
            server.tick += 1;
            server.think_and_move();
            let off = off(&server);
            assert!((-1.0..2.0).contains(&off.x) && (-1.0..2.0).contains(&off.z), "the mouflon got out of the fenced pen {t} ticks on: at {off:.2} from its middle cell");
        }
        let open = server.content.blocks.expect("base:oak_fence_gate_open_east");
        assert_eq!(server.content.blocks.swapped(server.content.blocks.expect("base:oak_fence_gate_east")), Some(open), "the gate swings open");
        server.set_block(middle + IVec3::new(-2, 0, 0), open);
        let out = (0..15 * TICKS_PER_SECOND as u64).find(|_| {
            server.tick += 1;
            server.think_and_move();
            off(&server).x < -2.0
        });
        assert!(out.is_some(), "the mouflon never came out through the open gate after the wheat: at {:.2}", off(&server));
    }

    /// Prints the table (`cargo test -p voxmar_server --lib fight_table -- --ignored --nocapture`).
    #[test]
    #[ignore = "a measurement: prints a table"]
    fn fight_table() {
        let only = std::env::var("VOXMAR_SCENE").ok();
        for (scene, set) in scenes().into_iter().chain(ground_scenes()).filter(|(s, _)| only.as_deref().is_none_or(|o| o == *s)) {
            for name in FIGHTERS.into_iter().chain(PREY) {
                let f = fight(name, set, 20.0);
                println!("{scene:>10} {name:>16}: first {:>5} strikes {:>2} pinned {:>4.1} s pressing {:>4.1} s cornered {:>4.1} s hops {:>2} nearest {:.2}", f.first.map_or("never".into(), |s| format!("{s:.1}")), f.strikes, f.pinned, f.pressing, f.cornered, f.hops, f.nearest);
            }
        }
    }
}

/// Findings from review on the ways creatures find and follow: a way followed to its end, a
/// climb, the shore search out of range of any shore, a home led through water.
#[cfg(test)]
mod review_tests {
    use super::cornered_tests::{join, pen, put, wall};
    use super::*;

    /// A wolf had a way to a cell beyond a wall and followed it to its end; meeting the wall
    /// again on the way to much the same cell, it looks for a way again. Before: the old
    /// way's goal stood for the new one, so it only ran along the wall.
    #[test]
    fn a_way_followed_to_its_end_is_looked_for_again_at_the_wall() {
        let mut server = crate::test_server();
        let c = pen(&mut server, 14);
        wall(&mut server, (-13..=13).filter(|&z| z != 8).map(|z| c + IVec3::new(0, 0, z)), 3);
        let id = put(&mut server, "base:wolf", c.as_vec3() + Vec3::new(-3.5, 0.0, 0.5), Vec3::X);
        let goal = c.as_vec3() + Vec3::new(4.5, 0.0, 0.5);
        let creature = server.creatures.live.get_mut(&id).unwrap();
        (creature.way_to, creature.way_at) = (Some(goal.floor().as_ivec3()), 0);
        server.tick = 1000;
        let run = server.content.creatures.get(server.creatures.live[&id].species).def.movement.run;
        let mut found = false;
        for _ in 0..3 * TICKS_PER_SECOND {
            server.tick += 1;
            server.creatures.live.get_mut(&id).unwrap().decision = Decision { goal: Some(goal), speed: run, ..Default::default() };
            server.move_creature(id, 1.0 / TICKS_PER_SECOND as f32);
            found |= !server.creatures.live[&id].way.is_empty();
        }
        assert!(found, "it never looked for the door again, at {}", server.creatures.live[&id].body.position - c.as_vec3());
    }

    /// A rabbit below a one-block step, on a way up it and on along the top: the cell up
    /// the step is not passed until it has climbed. Before: within a block and a fifth of
    /// its level, the cell a block up counted as reached from the ground below.
    #[test]
    fn a_cell_a_block_up_is_not_passed_from_below() {
        let mut server = crate::test_server();
        let c = pen(&mut server, 6);
        wall(&mut server, (1..=3).flat_map(|x| (-2..=3).map(move |z| c + IVec3::new(x, 0, z))), 1);
        let id = put(&mut server, "base:rabbit", c.as_vec3() + Vec3::new(0.8, 0.0, 0.9), Vec3::X);
        let up = [IVec3::new(1, 1, 0), IVec3::new(1, 1, 1), IVec3::new(1, 1, 2)].map(|cell| c + cell);
        let walk = server.content.creatures.get(server.creatures.live[&id].species).def.movement.walk;
        let creature = server.creatures.live.get_mut(&id).unwrap();
        (creature.way, creature.way_to, creature.way_at, creature.detour) = (up.to_vec(), Some(up[2]), 0, 1.0);
        creature.decision = Decision { goal: Some(up[2].as_vec3() + Vec3::new(0.5, 0.0, 0.5)), speed: walk, ..Default::default() };
        server.tick = 10;
        server.move_creature(id, 1.0 / TICKS_PER_SECOND as f32);
        let creature = &server.creatures.live[&id];
        assert!(creature.body.position.y < c.y as f32 + 0.5, "the fixture means it to be below the step");
        assert_eq!(creature.way.first(), Some(&up[0]), "it passed the cell up the step from below");
    }

    /// A wall with its doorway (two across, wide enough for a crocodile) sunk a block and
    /// flooded (a ford): a capybara, a crocodile and a mouflon on a way through it follow it
    /// across. Before: the bobbing body's feet stood at the water's top, a block over the
    /// ford's cells, so no cell counted as reached and it hovered at the first (a crocodile
    /// stood still 230 of 400 ticks); and a mouflon bobbing there counted itself out of its
    /// depth, made for the shore behind it and pressed the wall.
    #[test]
    fn a_way_through_a_ford_is_followed_across() {
        for name in ["base:capybara", "base:crocodile", "base:mouflon"] {
            let mut server = crate::test_server();
            let c = pen(&mut server, 14);
            wall(&mut server, (-13..=13).filter(|z| !(3..=4).contains(z)).map(|z| c + IVec3::new(1, 0, z)), 3);
            let (stone, water) = (server.content.blocks.expect("base:stone"), server.content.blocks.expect("base:water"));
            for (x, z) in (-1..=3).flat_map(|x| (3..=4).map(move |z| (x, z))) {
                server.world.set_block(c + IVec3::new(x, -1, z), water);
                server.world.set_block(c + IVec3::new(x, -2, z), stone);
            }
            let id = put(&mut server, name, c.as_vec3() + Vec3::new(-3.5, 0.0, 0.5), Vec3::X);
            let goal = c.as_vec3() + Vec3::new(5.5, 0.0, 0.5);
            let walk = server.content.creatures.get(server.creatures.live[&id].species).def.movement.walk;
            server.tick = 1000;
            let across = (0..10 * TICKS_PER_SECOND as u64).find(|_| {
                server.tick += 1;
                server.creatures.live.get_mut(&id).unwrap().decision = Decision { goal: Some(goal), speed: walk, ..Default::default() };
                server.move_creature(id, 1.0 / TICKS_PER_SECOND as f32);
                server.creatures.live[&id].body.position.x > c.x as f32 + 2.5
            });
            assert!(across.is_some(), "{name} never got across the ford: at {}", server.creatures.live[&id].body.position - c.as_vec3());
        }
    }

    /// A mouflon in the middle of a lake two deep with no shore in its range looks for one
    /// once a second, not every tick. Before: a search that found nothing was tried again
    /// the next tick.
    #[test]
    fn out_of_its_depth_with_no_shore_near_it_looks_once_a_second() {
        let mut server = crate::test_server();
        let c = pen(&mut server, 14);
        let water = server.content.blocks.expect("base:water");
        for z in -13..=13 {
            for x in -13..=13 {
                for y in [-1, 0] {
                    server.world.set_block(c + IVec3::new(x, y, z), water);
                }
            }
        }
        let id = put(&mut server, "base:mouflon", c.as_vec3() + Vec3::new(0.5, -0.5, 0.5), Vec3::X);
        server.tick = 1000;
        let (mut looked, mut last) = (0, None);
        for _ in 0..3 * TICKS_PER_SECOND {
            server.tick += 1;
            server.think_and_move();
            let creature = &server.creatures.live[&id];
            assert!(creature.shore.is_none(), "the fixture has no shore in range");
            if creature.shore_at.is_some() && creature.shore_at != last {
                looked += 1;
                last = creature.shore_at;
            }
        }
        assert!((3..=4).contains(&looked), "it looked {looked} times in 3 s");
    }

    /// A rabbit led by wild carrot through a pool a block deep: its home goes with it on the
    /// dry ground, never into the water. Before: a think in the pool made its home there.
    #[test]
    fn a_rabbit_led_through_water_keeps_a_dry_home() {
        let mut server = crate::test_server();
        let c = pen(&mut server, 14);
        let (stone, water) = (server.content.blocks.expect("base:stone"), server.content.blocks.expect("base:water"));
        for z in -13..=13 {
            for x in -2..=2 {
                server.world.set_block(c + IVec3::new(x, -1, z), water);
                server.world.set_block(c + IVec3::new(x, -2, z), stone);
            }
        }
        let player = join(&mut server, c.as_vec3() + Vec3::new(-5.5, 0.0, 0.5));
        server.players.get_mut(&player).unwrap().inventory.slots[0] = Some(ItemStack::new(server.content.blocks.expect("base:wild_carrot"), 1));
        let id = put(&mut server, "base:rabbit", c.as_vec3() + Vec3::new(-8.5, 0.0, 0.5), Vec3::X);
        server.tick = 1000;
        let mut waded = false;
        for t in 0..20 * TICKS_PER_SECOND as u64 {
            server.tick += 1;
            // The player walks east across the pool and on, the rabbit after the carrot.
            let p = server.players[&player].position;
            if p.x < c.x as f32 + 8.5 {
                server.players.get_mut(&player).unwrap().position = p + Vec3::X * 1.5 / TICKS_PER_SECOND as f32;
            }
            server.think_and_move();
            let creature = &server.creatures.live[&id];
            waded |= creature.body.in_water;
            let home = creature.home.floor().as_ivec3();
            let ground = Ground::of(&server);
            assert!(!ground.water_at(home) && !ground.water_at(home - IVec3::Y), "its home went over the water at {} (tick {t})", creature.home - c.as_vec3());
        }
        assert!(waded, "the fixture means it to be led through the pool: at {}", server.creatures.live[&id].body.position - c.as_vec3());
    }

    /// Whether a grazed tuft goes for good is drawn by the world's count of rolls, never the
    /// tick it is grazed on: the same grazing later goes the same way, and each counts on.
    #[test]
    fn a_grazed_tuft_goes_by_the_worlds_count_not_the_tick() {
        let grazed = |tick: u64| {
            let mut server = crate::test_server();
            server.meta.draw_key = 0x0d1a;
            let at = server.spawn.floor().as_ivec3();
            server.load_area(at - IVec3::new(2, 0, 2), at + IVec3::new(26, 0, 2));
            let species = server.content.creatures.id("base:rabbit").expect("rabbits");
            let id = server.spawn_creature(species, server.spawn, 1).expect("room");
            let grass = server.content.blocks.expect("base:short_grass");
            server.tick = tick;
            let mut gone = Vec::new();
            for x in 0..24 {
                let cell = at + IVec3::new(x, 0, 0);
                server.set_block(cell, grass);
                let neighbours = Neighbours::of(&server.creatures.live);
                server.carry_out(id, Decision { eat: Some(cell), ..Default::default() }, &neighbours);
                gone.push(server.block(cell) != Some(grass));
                server.tick += 37;
            }
            (gone, server.meta.rolls)
        };
        let first = grazed(1_000);
        assert_eq!(first.1, 24, "each grazing counted on");
        assert!(first.0.iter().any(|&g| g) && first.0.iter().any(|&g| !g), "24 grazings all went alike: {:?}", first.0);
        assert_eq!(grazed(1_377), first, "the same grazing later went otherwise");
    }

    /// A kill's loot is drawn by the killer's own count of kills, never the tick it dies on
    /// nor the world's loot rolls before it: the same kills later, after others' deaths
    /// rolled, drop alike, and each counts on.
    #[test]
    fn a_kills_loot_goes_by_the_killers_count_not_the_tick() {
        let killed = |tick: u64, before: u32| {
            let mut server = crate::test_server();
            server.meta.draw_key = 0x0d1a;
            let at = server.spawn.floor().as_ivec3();
            server.load_area(at - IVec3::new(2, 0, 2), at + IVec3::new(2, 0, 2));
            let (_client, connection) = voxmar_protocol::local_pair();
            let mut player = crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
            player.joined = true;
            server.players.insert(1, player);
            for _ in 0..before {
                server.meta.next_loot();
            }
            let species = server.content.creatures.id("base:rabbit").expect("rabbits");
            server.tick = tick;
            let mut found = Vec::new();
            for _ in 0..12 {
                let id = server.spawn_creature(species, server.spawn, 1).expect("room");
                server.creatures.live.get_mut(&id).unwrap().hurt_by = Some(1);
                server.creature_dies(id);
                // Due out once the body has fallen.
                let blocks = &server.content.blocks;
                let mut drops: Vec<(String, u8)> = server.creatures.falling_loot.iter().map(|(_, stack, _)| (blocks.def(stack.item).name.clone(), stack.count)).collect();
                drops.sort();
                found.push(drops);
                server.tick += 37;
            }
            (found, server.players[&1].draws.kills)
        };
        let first = killed(1_000, 0);
        assert!(first.1 >= 12, "each kill's loot drawn on the killer's count: {} draws", first.1);
        let total = |drops: &Vec<(String, u8)>| drops.iter().map(|(_, n)| u32::from(*n)).sum::<u32>();
        let gains: Vec<u32> = first.0.windows(2).map(|w| total(&w[1]) - total(&w[0])).collect();
        assert!(gains.iter().any(|&g| g != gains[0]), "12 kills all dropped alike: {:?}", first.0.last());
        assert_eq!(killed(1_377, 3), first, "the same kills later, after other loot rolls, dropped otherwise");
    }
}
