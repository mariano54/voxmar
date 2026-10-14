//! Seats are data (a `seat` tag; base's `sit` action and `fireside` effect); these tests
//! sit a player on one through the server's actions: set on it with a lower eye, By the
//! Fire beside a campfire, and up again once they step off or the seat goes.

use std::sync::Arc;
use std::time::Instant;

use voxmar_engine::action::ActionTarget;
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::{ColumnPos, Face};
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::world::Column;

use crate::Server;
use crate::process::Process;

const FLOOR: i32 = 199;
/// The stool, two blocks from the player, with a campfire beyond it.
const STOOL: IVec3 = IVec3::new(9, FLOOR + 1, 8);
const FIRE: IVec3 = IVec3::new(11, FLOOR + 1, 8);

/// A stone floor with a stool on it, and player 1 standing by it, empty-handed.
fn fixture() -> Server {
    let mut server = crate::test_server();
    server.world = Default::default();
    let stone = id(&server, "base:stone");
    let mut column = Column::default();
    for x in 0..32 {
        for z in 0..32 {
            column.set_block(IVec3::new(x, FLOOR, z), stone);
        }
    }
    server.world.insert(ColumnPos::new(0, 0), Arc::new(column));
    let stool = id(&server, "base:oak_stool");
    server.set_block(STOOL, stool);
    let (client, connection) = voxmar_protocol::local_pair();
    std::mem::forget(client);
    let at = Vec3::new(7.5, (FLOOR + 1) as f32, 8.5);
    let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
    player.joined = true;
    server.players.insert(1, player);
    server
}

fn id(server: &Server, name: &str) -> BlockId {
    server.content.blocks.expect(name)
}

fn sit(server: &mut Server, at: IVec3) {
    let action = server.content.actions.expect("base:sit");
    server.propose(1, action, ActionTarget::Block { block: at, face: Face::NegX }, Instant::now());
}

fn by_the_fire(server: &Server) -> bool {
    let fireside = server.content.effects.id("base:fireside").expect("base's fireside");
    server.players[&1].vitals.effects.iter().any(|a| a.effect == fireside)
}

/// Sat on, a stool sets the player on it, eye lower than standing there; with a campfire
/// near, they are By the Fire. Stepping off, they stand, and the fire's rest goes.
#[test]
fn a_stool_by_the_fire_is_sat_on_and_rests_until_stepped_off() {
    let mut server = fixture();
    let standing = server.players[&1].eye().y;
    sit(&mut server, STOOL);
    let player = &server.players[&1];
    assert_eq!(player.vitals.seated, Some(STOOL), "not sat on the stool");
    assert_eq!(player.position, Vec3::new(9.5, (FLOOR + 1) as f32, 8.5), "not set in the stool");
    assert!(player.eye().y < standing - 0.25, "the eye sat on a stool ({}) is not lower than standing beside it ({standing})", player.eye().y);
    crate::process::Survival.run(&mut server);
    assert_eq!(server.players[&1].vitals.seated, Some(STOOL));
    assert!(!by_the_fire(&server), "By the Fire with no fire");
    let campfire = id(&server, "base:campfire");
    server.set_block(FIRE, campfire);
    crate::process::Survival.run(&mut server);
    assert!(by_the_fire(&server), "sat by a campfire, not By the Fire");
    // A step off the stool, and they are standing again.
    let player = server.players.get_mut(&1).unwrap();
    player.position = Vec3::new(8.5, (FLOOR + 1) as f32, 8.5);
    crate::process::Survival.run(&mut server);
    assert_eq!(server.players[&1].vitals.seated, None, "still sat after stepping off");
    assert!(!by_the_fire(&server), "By the Fire standing up");
}

/// A seat broken from under a player stands them up; one with a block over it, or asked
/// with a block to place in hand, is not sat on (the block goes against it instead).
#[test]
fn a_seat_wants_room_and_an_empty_hand() {
    let mut server = fixture();
    sit(&mut server, STOOL);
    server.set_block(STOOL, BlockId::AIR);
    crate::process::Survival.run(&mut server);
    assert_eq!(server.players[&1].vitals.seated, None, "sat on a stool that is gone");
    let stool = id(&server, "base:oak_stool");
    server.set_block(STOOL, stool);
    let stone = id(&server, "base:stone");
    server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack { item: stone, count: 1 });
    sit(&mut server, STOOL);
    assert_eq!(server.players[&1].vitals.seated, None, "sat with a block to place in hand");
    server.players.get_mut(&1).unwrap().inventory.slots[0] = None;
    server.set_block(STOOL + IVec3::Y, stone);
    sit(&mut server, STOOL);
    assert_eq!(server.players[&1].vitals.seated, None, "sat under a block, with no room");
    server.set_block(STOOL + IVec3::Y, BlockId::AIR);
    // A roof two blocks over the floor, as a house's is: room enough to sit.
    server.set_block(STOOL + IVec3::new(0, 2, 0), stone);
    sit(&mut server, STOOL);
    assert_eq!(server.players[&1].vitals.seated, Some(STOOL), "not sat on a stool under a roof two blocks up");
}

/// Sat with a table beside the stool, the player is At the Table: a meal there heals a
/// quarter more and its good lasts a quarter longer. Standing by the table, it does not.
#[test]
fn a_meal_at_the_table_does_more() {
    let mut server = fixture();
    let table = id(&server, "base:oak_table");
    server.set_block(STOOL + IVec3::new(1, 0, 0), table);
    let at_table = server.content.effects.id("base:at_table").expect("base's at_table");
    let has = |server: &Server| server.players[&1].vitals.effects.iter().any(|a| a.effect == at_table);
    crate::process::Survival.run(&mut server);
    assert!(!has(&server), "At the Table standing beside it");
    sit(&mut server, STOOL);
    crate::process::Survival.run(&mut server);
    assert!(has(&server), "sat at a table, not At the Table");
    let modifiers = server.players[&1].modifiers;
    assert_eq!((modifiers.meal_heal, modifiers.meal_time), (1.25, 1.25));
}
