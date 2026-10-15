//! Charges as the land answers them (`creature::moves::Charge`, docs/bosses.md): each rule
//! tried on Old Tusk, on a stone floor of its own far from the world.

use std::sync::Arc;

use voxmar_engine::block::BlockId;
use voxmar_engine::combat::Defended;
use voxmar_engine::creature::moves::Charge;
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::world::Column;
use voxmar_protocol::{ServerMessage, TICKS_PER_SECOND};

use crate::Server;

/// The floor's top: creatures and the player stand at `FLOOR + 1`.
const FLOOR: i32 = 199;
const STAND: f32 = FLOOR as f32 + 1.0;

/// A floor three blocks of stone thick under columns 0..3 each way, player 1 standing on it
/// at `at` facing `look` (a flat direction), and Old Tusk at `tusk` facing the player.
struct Field {
    server: Server,
    client: voxmar_protocol::ClientConnection,
    tusk: u32,
}

impl Field {
    fn new(at: Vec3, tusk: Vec3) -> Self {
        let mut server = crate::test_server();
        server.world = Default::default();
        let stone = server.content.blocks.expect("base:stone");
        for cx in 0..3 {
            for cz in 0..3 {
                let mut column = Column::default();
                for x in 0..32 {
                    for z in 0..32 {
                        for y in FLOOR - 2..=FLOOR {
                            column.set_block(IVec3::new(x, y, z), stone);
                        }
                    }
                }
                server.world.insert(ColumnPos::new(cx, cz), Arc::new(column));
            }
        }
        let (client, connection) = voxmar_protocol::local_pair();
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        let to = (tusk - at).with_y(0.0);
        player.yaw = voxmar_engine::creature::motion::yaw_of(to);
        player.pitch = 0.0;
        server.players.insert(1, player);
        let species = server.content.creatures.id("base:old_tusk").expect("Old Tusk");
        let tusk = server.place_creature(species, tusk, 9, 0.0).expect("placed");
        let live = server.creatures.live.get_mut(&tusk).unwrap();
        live.scale = 1.0;
        live.body.yaw = voxmar_engine::creature::motion::yaw_of(-to);
        Self { server, client, tusk }
    }

    /// The charge of his first phase.
    fn charge(&self) -> Charge {
        let species = self.server.content.creatures.get(self.server.creatures.live[&self.tusk].species);
        let index = species.def.moves.iter().position(|m| m.name == "charge").expect("his charge") as u8 + 1;
        species.attack_at(index).and_then(|a| a.charge).expect("a charge")
    }

    /// He sets off at player 1, as at the end of his charge's wind-up.
    fn sets_off(&mut self) {
        let species = self.server.content.creatures.get(self.server.creatures.live[&self.tusk].species);
        let index = species.def.moves.iter().position(|m| m.name == "charge").unwrap() as u8 + 1;
        self.server.creatures.live.get_mut(&self.tusk).unwrap().attack = index;
        let charge = self.charge();
        self.server.start_charge(self.tusk, 1, charge);
    }

    /// Ticks until his charge is over (or `seconds` pass), landing any blow it struck.
    fn run(&mut self, seconds: f32) {
        let dt = 1.0 / TICKS_PER_SECOND as f32;
        for _ in 0..(seconds * TICKS_PER_SECOND as f32) as u32 {
            self.server.tick += 1;
            if !self.server.creatures.live.contains_key(&self.tusk) {
                break;
            }
            self.server.move_creature(self.tusk, dt);
            self.server.tick_fights();
            if !self.charging() {
                break;
            }
        }
        self.server.tick_fights();
    }

    fn charging(&self) -> bool {
        self.server.creatures.live.get(&self.tusk).is_some_and(|c| c.charging.is_some())
    }

    fn at(&self) -> Vec3 {
        self.server.creatures.live[&self.tusk].body.position
    }

    fn health(&self) -> u16 {
        self.server.creatures.live.get(&self.tusk).map_or(0, |c| c.health)
    }

    /// Seconds he is held still for from now.
    fn held(&self) -> f32 {
        self.server.creatures.live[&self.tusk].held_until.saturating_sub(self.server.tick) as f32 / TICKS_PER_SECOND as f32
    }

    /// What the player was told of his blows.
    fn told(&self) -> Vec<Defended> {
        std::iter::from_fn(|| self.client.try_recv().ok().flatten())
            .filter_map(|message| match message {
                ServerMessage::Hurt { defended, .. } => Some(defended),
                _ => None,
            })
            .collect()
    }

    fn hold(&mut self, item: &str) {
        let item = self.server.content.blocks.expect(item);
        self.server.players.get_mut(&1).unwrap().inventory.add(item, 1);
    }

    fn set(&mut self, cell: IVec3, block: &str) {
        let block = if block == "air" { BlockId::AIR } else { self.server.content.blocks.expect(block) };
        self.server.world.set_block(cell, block);
    }
}

/// A charge runs straight at where the player stood and comes down on the first player in
/// its way, stopping there; it does not turn after one who stepped aside, but runs on past.
#[test]
fn a_charge_runs_straight_and_strikes_the_first_player_in_its_way() {
    let mut field = Field::new(Vec3::new(40.5, STAND, 48.5), Vec3::new(40.5, STAND, 38.5));
    field.sets_off();
    assert!(field.charging() && field.server.creatures.live[&field.tusk].view(field.tusk).pose == voxmar_engine::creature::brain::Pose::Attacking);
    field.run(3.0);
    assert!(!field.charging(), "the charge never ended");
    let told = field.told();
    assert!(matches!(told.as_slice(), [Defended::Hit | Defended::Missed]), "his blow was judged once, as a blow: {told:?}");
    let at = field.at();
    assert!((at.z - 48.5).abs() < 2.0 && at.z < 48.5, "he stopped at the player, not past them: {at}");
    assert!((at.x - 40.5).abs() < 0.05, "straight on: {at}");

    // Stepped aside as he comes: he runs on past, the whole of his charge.
    let mut field = Field::new(Vec3::new(40.5, STAND, 48.5), Vec3::new(40.5, STAND, 30.5));
    field.sets_off();
    field.server.players.get_mut(&1).unwrap().position.x = 43.5;
    field.run(4.0);
    assert!(field.told().is_empty(), "no blow on one who stepped aside");
    let at = field.at();
    assert!((at.x - 40.5).abs() < 0.05, "he did not turn after them: {at}");
    assert!(at.z - 30.5 > field.charge().distance - 1.5, "he ran his whole charge: {at}");
}

/// A spear's guard raised facing a charge braces it: he runs onto the point, takes the
/// spear's max hit and a counter's share more, sure to land, and stands dazed; the player
/// takes nothing. A sword's guard does not brace, and the heavy charge goes through it.
#[test]
fn a_braced_spear_stops_a_charge_on_its_point() {
    let mut field = Field::new(Vec3::new(40.5, STAND, 48.5), Vec3::new(40.5, STAND, 40.5));
    field.hold("base:flint_spear");
    field.server.raise_guard(1, true);
    let (before, player_before) = (field.health(), field.server.players[&1].health.current);
    field.sets_off();
    field.run(3.0);
    let spear = *field.server.content.combat.weapon(field.server.content.blocks.expect("base:flint_spear")).expect("the flint spear is a weapon");
    let feel = field.server.content.combat.feel;
    let point = ((spear.max_hit(1) as f32 * (1.0 + feel.counter_max_hit)).round() as u16).max(1);
    assert_eq!(before - field.health(), point, "he ran onto the point");
    assert_eq!(field.server.players[&1].health.current, player_before, "the bracer took nothing");
    assert_eq!(field.told(), vec![Defended::Perfect]);
    assert!(field.held() >= field.charge().stun, "he stands dazed for his charge's stun ({} s)", field.held());
    assert_eq!(field.server.creatures.live[&field.tusk].tell(), voxmar_protocol::STAGGERED, "dazed on the point, he reels");

    let mut field = Field::new(Vec3::new(40.5, STAND, 48.5), Vec3::new(40.5, STAND, 40.5));
    field.hold("base:stone_sword");
    field.server.raise_guard(1, true);
    let before = field.health();
    field.sets_off();
    field.run(3.0);
    assert_eq!(field.health(), before, "a sword does not brace");
    let told = field.told();
    assert!(matches!(told.as_slice(), [Defended::Hit | Defended::Missed]), "the heavy charge went through the sword's guard: {told:?}");
}

/// A wall two blocks high stops a charge, the charger dazed for its stun; a single step it
/// hops, and runs on.
#[test]
fn a_charge_into_a_wall_dazes_the_charger_and_hops_a_step() {
    let mut field = Field::new(Vec3::new(40.5, STAND, 70.0), Vec3::new(40.5, STAND, 36.5));
    for x in 36..46 {
        field.set(IVec3::new(x, FLOOR + 1, 40), "base:stone");
        field.set(IVec3::new(x, FLOOR + 1, 47), "base:stone");
        field.set(IVec3::new(x, FLOOR + 2, 47), "base:stone");
    }
    field.sets_off();
    field.run(4.0);
    let at = field.at();
    assert!(at.z > 41.0 && at.z < 47.0, "he hopped the step and stopped at the wall: {at}");
    assert!(field.held() >= field.charge().stun, "the wall dazed him ({} s)", field.held());
    assert_eq!(field.server.creatures.live[&field.tusk].tell(), voxmar_protocol::STAGGERED, "dazed, he reels, and players see it");
    // Then he waits out only the rest of the charge's `every`, as after a blow that lands.
    let live = &field.server.creatures.live[&field.tusk];
    let attack = field.server.content.creatures.get(live.species).attack_at(live.attack).unwrap();
    let rest = attack.every - attack.windup.unwrap() - attack.recovery.unwrap();
    assert!((live.mind.cooldown - rest).abs() < 1e-4, "cooldown {} after the daze, not {rest}", live.mind.cooldown);
}

/// A stair of one-deep steps is no wall: in the air between hops his snout meets the next
/// riser and he falls back onto the step, and that is not counted as stalled, so he is not
/// dazed; the charge ends on its time. (How far up he gets is the hop's, `motion::step`.)
#[test]
fn a_charge_runs_up_a_stair_of_one_deep_steps() {
    let mut field = Field::new(Vec3::new(40.5, STAND, 70.0), Vec3::new(40.5, STAND, 36.5));
    for x in 36..46 {
        for (i, z) in (40..44).enumerate() {
            for y in 0..=i as i32 {
                field.set(IVec3::new(x, FLOOR + 1 + y, z), "base:stone");
            }
        }
        for z in 44..56 {
            for y in 1..=4 {
                field.set(IVec3::new(x, FLOOR + y, z), "base:stone");
            }
        }
    }
    let stun = field.charge().stun;
    field.sets_off();
    field.run(4.0);
    let at = field.at();
    assert!(!field.charging(), "his charge ended, at {at}");
    assert!(field.held() < stun, "the stair dazed him ({} s held), at {at}", field.held());
}

/// A charge into deep water ends once its distance is spent, swimming as on foot: he does
/// not run on, held from thinking, to the far shore.
#[test]
fn a_charge_into_deep_water_ends_when_its_distance_is_spent() {
    let mut field = Field::new(Vec3::new(40.5, STAND, 90.0), Vec3::new(40.5, STAND, 36.5));
    for x in 20..62 {
        for z in 40..96 {
            field.set(IVec3::new(x, FLOOR, z), "base:water");
            field.set(IVec3::new(x, FLOOR - 1, z), "base:water");
        }
    }
    let distance = field.charge().distance;
    field.sets_off();
    field.run(12.0);
    let at = field.at();
    assert!(!field.charging(), "his charge ended in the water, at {at}");
    assert!(at.z - 36.5 < distance + 4.0, "he ran {} blocks of a {distance}-block charge", at.z - 36.5);
}

/// A charge runs off a cliff blind, and the fall costs the charger a tenth of its health
/// for each block past the safe fall; into a pit two deep it falls unhurt, and cannot climb
/// out after the player at its edge.
#[test]
fn a_charge_off_a_cliff_hurts_and_a_pit_holds_the_charger() {
    let mut field = Field::new(Vec3::new(40.5, STAND, 75.0), Vec3::new(40.5, STAND, 36.5));
    // Past z 40 the floor drops ten blocks.
    for x in 30..52 {
        for z in 40..64 {
            for y in FLOOR - 9..=FLOOR {
                field.set(IVec3::new(x, y, z), "air");
            }
            field.set(IVec3::new(x, FLOOR - 10, z), "base:stone");
        }
    }
    let most = field.server.content.creatures.get(field.server.creatures.live[&field.tusk].species).def.health;
    field.sets_off();
    field.run(4.0);
    let fall = 10.0 - field.server.survival.safe_fall;
    let lost = (fall.ceil() * field.server.content.combat.feel.charge_fall * most as f32).round() as u16;
    assert_eq!(most - field.health(), lost, "the fall cost him a tenth a block past the safe fall");
    // His own fall: the player he charged struck nothing, so is shown no hit and earns
    // nothing for it.
    let heard: Vec<ServerMessage> = std::iter::from_fn(|| field.client.try_recv().ok().flatten()).collect();
    assert!(!heard.iter().any(|m| matches!(m, ServerMessage::Struck { .. } | ServerMessage::Xp { .. })), "his fall was no blow of the player's: {heard:?}");

    // A pit two deep across his way, the player at its far edge.
    let mut field = Field::new(Vec3::new(40.5, STAND, 45.5), Vec3::new(40.5, STAND, 36.5));
    for x in 36..46 {
        for z in 40..44 {
            field.set(IVec3::new(x, FLOOR, z), "air");
            field.set(IVec3::new(x, FLOOR - 1, z), "air");
        }
    }
    field.sets_off();
    field.run(4.0);
    for _ in 0..TICKS_PER_SECOND {
        field.server.tick += 1;
        field.server.move_creature(field.tusk, 1.0 / TICKS_PER_SECOND as f32);
    }
    let at = field.at();
    assert!(at.y < STAND - 1.5 && at.z > 40.0 && at.z < 44.0, "he fell into the pit: {at}");
    assert_eq!(field.health(), most, "two blocks is a safe fall");
    // He goes after the player at the edge, and cannot climb out.
    let run = field.server.content.creatures.get(field.server.creatures.live[&field.tusk].species).def.movement.run;
    let live = field.server.creatures.live.get_mut(&field.tusk).unwrap();
    live.held_until = 0;
    live.decision.goal = Some(Vec3::new(40.5, STAND, 45.5));
    live.decision.speed = run;
    for _ in 0..10 * TICKS_PER_SECOND {
        field.server.tick += 1;
        field.server.move_creature(field.tusk, 1.0 / TICKS_PER_SECOND as f32);
    }
    let at = field.at();
    assert!(at.y < STAND - 0.9 && at.z < 44.0, "he climbed out: {at}");
}

/// Struck from afar, he does not flee but turns on the one who struck him and charges:
/// a boss fights to the end (his `Defend`, `Creature::fights`).
#[test]
fn struck_old_tusk_turns_and_charges() {
    let mut field = Field::new(Vec3::new(40.5, STAND, 36.5), Vec3::new(40.5, STAND, 46.5));
    field.server.hurt_creature(1, field.tusk, 1, None, voxmar_engine::combat::Blow::Plain);
    let mind = &field.server.creatures.live[&field.tusk].mind;
    assert!(mind.anger > 0.0 && mind.fear == 0.0, "angered, not frightened");
    // The fight clock follows the game's, as a player's stamped messages make it.
    let mut charged = false;
    for n in 1..=6 * TICKS_PER_SECOND {
        field.server.tick += 1;
        field.server.arrival = field.server.fights.epoch + std::time::Duration::from_secs_f64(n as f64 / TICKS_PER_SECOND as f64);
        field.server.tick_creatures();
        field.server.tick_fights();
        charged |= field.charging();
    }
    assert!(charged, "he charged the one who struck him");
}

/// His charge runs the way he faced when its aim locked: the blow that sets him off waits
/// out the player's round trip (`lag_allowance`) past the wind-up, and in that wait he does
/// not think, turn or walk after a player who stepped aside, so the line they read holds.
#[test]
fn a_charge_runs_the_line_its_aim_locked_on_through_the_lag_wait() {
    let mut field = Field::new(Vec3::new(40.5, STAND, 36.5), Vec3::new(40.5, STAND, 46.5));
    let lag = field.server.content.combat.feel.lag_allowance;
    assert!(lag > 0.0, "a wait to hold through");
    field.server.players.get_mut(&1).unwrap().round_trip = Some(lag);
    field.server.hurt_creature(1, field.tusk, 1, None, voxmar_engine::combat::Blow::Plain);
    let index = {
        let species = field.server.content.creatures.get(field.server.creatures.live[&field.tusk].species);
        species.def.moves.iter().position(|m| m.name == "charge").unwrap() as u8 + 1
    };
    let mut locked: Option<Vec3> = None;
    for n in 1..=10 * TICKS_PER_SECOND {
        field.server.tick += 1;
        field.server.arrival = field.server.fights.epoch + std::time::Duration::from_secs_f64(n as f64 / TICKS_PER_SECOND as f64);
        field.server.tick_creatures();
        field.server.tick_fights();
        let live = &field.server.creatures.live[&field.tusk];
        if let Some(charging) = live.charging {
            let along = locked.expect("his aim locked before he set off");
            assert!(charging.along.dot(along) > 0.999, "he ran {:?}, not the line he locked on, {along:?}", charging.along);
            return;
        }
        // His aim locked (the tracking window over, the wind-up under way): the player
        // steps well aside, and the rest of the wind-up and the lag wait follow.
        if locked.is_none() && live.winding && live.attack == index && live.tracking.is_none_or(|(_, until)| field.server.tick >= until) {
            locked = Some(live.body.forward());
            field.server.players.get_mut(&1).unwrap().position.x += 5.0;
        }
    }
    panic!("he never charged");
}
