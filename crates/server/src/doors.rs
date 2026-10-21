//! Doors are data (`swaps` and `upper` on a block, the engine's `swapping` and
//! `supported_by`); these tests drive one through the server's actions: placed whole,
//! opened and shut here first, refused shut on whoever stands in it, and broken whole;
//! and a shuttered window, which swaps the same way; and a double gate, two leaves set
//! side by side (`beside`) that go in, turn and break together.

use std::sync::Arc;
use std::time::Instant;

use voxmar_engine::action::ActionTarget;
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::{ColumnPos, Face};
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::world::Column;

use crate::Server;

const FLOOR: i32 = 199;
/// Where the door's lower half goes: on the floor, three blocks from the player.
const DOOR: IVec3 = IVec3::new(10, FLOOR + 1, 8);

/// A stone floor, and player 1 standing on it holding a door.
fn fixture() -> Server {
    let mut server = crate::test_server();
    server.world = Default::default();
    let stone = server.content.blocks.expect("base:stone");
    let mut column = Column::default();
    for x in 0..32 {
        for z in 0..32 {
            column.set_block(IVec3::new(x, FLOOR, z), stone);
        }
    }
    server.world.insert(ColumnPos::new(0, 0), Arc::new(column));
    let (client, connection) = voxmar_protocol::local_pair();
    std::mem::forget(client);
    let at = Vec3::new(7.5, (FLOOR + 1) as f32, 8.5);
    let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
    player.joined = true;
    let door = id(&server, "base:wooden_door");
    player.inventory.slots[0] = Some(ItemStack::new(door, 1));
    player.selected = 0;
    server.players.insert(1, player);
    server
}

fn id(server: &Server, name: &str) -> BlockId {
    server.content.blocks.expect(name)
}

fn act(server: &mut Server, action: &str, target: ActionTarget) {
    let action = server.content.actions.expect(action);
    server.propose(1, action, target, Instant::now());
}

/// The door's two cells, by name.
fn door(server: &Server) -> [String; 2] {
    [DOOR, DOOR + IVec3::Y].map(|at| server.block(at).map_or_else(String::new, |b| server.content.blocks.def(b).name.clone()))
}

fn place(server: &mut Server) {
    act(server, "core:place", ActionTarget::Block { block: DOOR - IVec3::Y, face: Face::PosY });
}

/// Placed, both halves go in facing the player (west of it); opened, both turn open and
/// let anyone through; shut, both are solid again, the server's world first.
#[test]
fn a_door_is_placed_whole_and_opens_and_shuts() {
    let mut server = fixture();
    place(&mut server);
    assert_eq!(door(&server), ["base:wooden_door_west", "base:wooden_door_top_west"]);
    assert!(server.is_solid(DOOR) && server.is_solid(DOOR + IVec3::Y));
    // Used by its top half too.
    act(&mut server, "base:open", ActionTarget::Block { block: DOOR + IVec3::Y, face: Face::NegX });
    assert_eq!(door(&server), ["base:wooden_door_open_west", "base:wooden_door_open_top_west"]);
    assert!(!server.is_solid(DOOR) && !server.is_solid(DOOR + IVec3::Y));
    act(&mut server, "base:open", ActionTarget::Block { block: DOOR, face: Face::NegX });
    assert_eq!(door(&server), ["base:wooden_door_west", "base:wooden_door_top_west"]);
}

/// A door needs room for both halves, and does not shut on someone standing in it.
#[test]
fn a_door_needs_room_and_never_shuts_on_anyone() {
    let mut server = fixture();
    let stone = id(&server, "base:stone");
    server.set_block(DOOR + IVec3::Y, stone);
    place(&mut server);
    assert_eq!(door(&server)[0], "core:air", "no room for its top half");
    server.set_block(DOOR + IVec3::Y, BlockId::AIR);
    place(&mut server);
    act(&mut server, "base:open", ActionTarget::Block { block: DOOR, face: Face::NegX });
    // Stood in the doorway, the player keeps it open.
    server.players.get_mut(&1).unwrap().position = DOOR.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
    act(&mut server, "base:open", ActionTarget::Block { block: DOOR, face: Face::NegX });
    assert_eq!(door(&server)[0], "base:wooden_door_open_west");
}

/// Broken by either half, the whole door goes and gives one door back.
#[test]
fn a_door_breaks_whole() {
    for half in [DOOR, DOOR + IVec3::Y] {
        let mut server = fixture();
        place(&mut server);
        let door_item = id(&server, "base:wooden_door");
        let at = Instant::now();
        let mine = server.content.actions.expect("core:mine");
        server.propose(1, mine, ActionTarget::Block { block: half, face: Face::NegX }, at);
        server.complete_actions(at + std::time::Duration::from_secs(30));
        assert_eq!(door(&server), ["core:air", "core:air"], "broken by {half}");
        let held: u32 = server.players[&1].inventory.slots.iter().flatten().filter(|s| s.item == door_item).map(|s| s.count as u32).sum();
        let blocks = server.content.blocks.clone();
        let dropped: u32 = server.items.saved(|b| blocks.def(b).name.clone()).iter().filter(|i| i.item == "base:wooden_door").map(|i| i.count as u32).sum();
        assert_eq!(held + dropped, 1, "one door back, broken by {half}");
    }
}

/// A shuttered lattice window turned east shuts with a right click and keeps its facing,
/// drops nothing and gives nothing; it opens again the same way, and it never shuts on
/// someone standing against it.
#[test]
fn shutters_on_a_lattice_shut_as_they_were_set_and_never_on_anyone() {
    let mut server = fixture();
    let open = id(&server, "base:birch_shuttered_window_east");
    server.set_block(DOOR, open);
    let held = server.players[&1].inventory.clone();
    let name = |server: &Server| door(server)[0].clone();
    act(&mut server, "base:open", ActionTarget::Block { block: DOOR, face: Face::NegX });
    assert_eq!(name(&server), "base:birch_shuttered_window_shut_east");
    assert!(server.items.saved(|b| server.content.blocks.def(b).name.clone()).is_empty(), "shutting the window dropped something");
    assert_eq!(server.players[&1].inventory, held, "shutting the window changed what the player holds");
    act(&mut server, "base:open", ActionTarget::Block { block: DOOR, face: Face::NegX });
    assert_eq!(name(&server), "base:birch_shuttered_window_east");
    // Stood against its open leaves, the player keeps it open.
    server.players.get_mut(&1).unwrap().position = DOOR.as_vec3() + Vec3::new(1.2, 0.0, 0.5);
    act(&mut server, "base:open", ActionTarget::Block { block: DOOR, face: Face::NegX });
    assert_eq!(name(&server), "base:birch_shuttered_window_east", "shut on the player standing in it");
}

/// A double gate's two cells, by name: the leaf set, and the cell beside it, to the right
/// of the player who set it facing them (west of them, so south of it: +z).
fn double_gate(server: &Server) -> [String; 2] {
    [DOOR, DOOR + IVec3::Z].map(|at| server.block(at).map_or_else(String::new, |b| server.content.blocks.def(b).name.clone()))
}

fn hold_double_gate(server: &mut Server) {
    let gate = id(server, "base:oak_double_gate");
    let player = server.players.get_mut(&1).unwrap();
    player.inventory.slots[0] = Some(ItemStack::new(gate, 1));
}

/// A double gate goes in as two leaves, the second to the right of whoever sets it, both
/// facing them; used by either leaf, both swing open and let anyone through, and shut
/// again together.
#[test]
fn a_double_gate_is_placed_as_two_leaves_that_open_and_shut_together() {
    let mut server = fixture();
    hold_double_gate(&mut server);
    place(&mut server);
    assert_eq!(double_gate(&server), ["base:oak_double_gate_west", "base:oak_double_gate_leaf_west"]);
    assert!(server.is_solid(DOOR) && server.is_solid(DOOR + IVec3::Z));
    act(&mut server, "base:open", ActionTarget::Block { block: DOOR + IVec3::Z, face: Face::NegX });
    assert_eq!(double_gate(&server), ["base:oak_double_gate_open_west", "base:oak_double_gate_leaf_open_west"]);
    assert!(!server.is_solid(DOOR) && !server.is_solid(DOOR + IVec3::Z));
    act(&mut server, "base:open", ActionTarget::Block { block: DOOR, face: Face::NegX });
    assert_eq!(double_gate(&server), ["base:oak_double_gate_west", "base:oak_double_gate_leaf_west"]);
}

/// A double gate needs room for both leaves; broken by either, both go and one double gate
/// comes back.
#[test]
fn a_double_gate_needs_room_for_both_leaves_and_breaks_whole() {
    let mut server = fixture();
    hold_double_gate(&mut server);
    let stone = id(&server, "base:stone");
    server.set_block(DOOR + IVec3::Z, stone);
    place(&mut server);
    assert_eq!(double_gate(&server)[0], "core:air", "no room for its second leaf");
    for leaf in [DOOR, DOOR + IVec3::Z] {
        let mut server = fixture();
        hold_double_gate(&mut server);
        place(&mut server);
        let gate = id(&server, "base:oak_double_gate");
        let at = Instant::now();
        let mine = server.content.actions.expect("core:mine");
        server.propose(1, mine, ActionTarget::Block { block: leaf, face: Face::NegX }, at);
        server.complete_actions(at + std::time::Duration::from_secs(30));
        assert_eq!(double_gate(&server), ["core:air", "core:air"], "broken by {leaf}");
        let held: u32 = server.players[&1].inventory.slots.iter().flatten().filter(|s| s.item == gate).map(|s| s.count as u32).sum();
        let blocks = server.content.blocks.clone();
        let dropped: u32 = server.items.saved(|b| blocks.def(b).name.clone()).iter().filter(|i| i.item == "base:oak_double_gate").map(|i| i.count as u32).sum();
        let others = server.items.saved(|b| blocks.def(b).name.clone()).iter().filter(|i| i.item != "base:oak_double_gate").count();
        assert_eq!((held + dropped, others), (1, 0), "one double gate back and nothing else, broken by {leaf}");
    }
}
