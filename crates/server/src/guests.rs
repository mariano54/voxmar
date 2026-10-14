//! A guest at another player's lit hearth: these tests stand player 1 by player 2's
//! hearthstone and run the survival tick, which gives them that home's comfort while the
//! host is here, never announced to them, and nothing once they leave or the host does.

use std::sync::Arc;

use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::world::Column;

use crate::Server;
use crate::process::Process;
use crate::player::Player;

const FLOOR: i32 = 199;
/// The host's hearthstone, their home just above it (as lighting it sets).
const HEARTH: IVec3 = IVec3::new(12, FLOOR + 1, 12);

/// A stone floor with a hearthstone on it, player 2 (the host) at home by it and player 1
/// (the guest) two blocks from it, with no home of their own.
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
    let hearthstone = id(&server, "base:hearthstone");
    server.set_block(HEARTH, hearthstone);
    join(&mut server, 1, Vec3::new(10.5, (FLOOR + 1) as f32, 12.5));
    join(&mut server, 2, Vec3::new(13.5, (FLOOR + 1) as f32, 12.5));
    server.players.get_mut(&2).unwrap().home = Some(HEARTH.as_vec3() + Vec3::new(0.5, 1.0, 0.5));
    server
}

fn join(server: &mut Server, who: u32, at: Vec3) {
    let (client, connection) = voxmar_protocol::local_pair();
    std::mem::forget(client);
    let mut player = Player::new(who, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
    player.joined = true;
    server.players.insert(who, player);
}

fn id(server: &Server, name: &str) -> BlockId {
    server.content.blocks.expect(name)
}

fn hearthside(server: &Server, who: u32) -> bool {
    let effect = server.content.effects.id("base:hearthside").expect("base's hearthside");
    server.players[&who].vitals.effects.iter().any(|a| a.effect == effect)
}

/// By a friend's hearth, the guest is Hearthside as the host is; it is not announced to
/// them, kept as reached or felt for their quests, so their own hearth does all that.
#[test]
fn a_guest_by_a_friends_hearth_is_hearthside_unannounced() {
    let mut server = fixture();
    crate::process::Survival.run(&mut server);
    assert!(hearthside(&server, 2), "the host is not Hearthside at their own hearth");
    assert!(hearthside(&server, 1), "a guest by the host's hearth is not Hearthside");
    let effect = server.content.effects.id("base:hearthside").unwrap();
    let (guest, host) = (&server.players[&1], &server.players[&2]);
    assert!(!guest.announced.contains(&effect), "a guest was told of a step of another's home");
    assert!(!guest.tallies.contains_key("felt:base:hearthside"), "another's home counted as felt for a guest's quests");
    assert!(host.announced.contains(&effect), "the host was not told of their own step");
    assert!(host.tallies.contains_key("felt:base:hearthside"));
}

/// The comfort is the guest's only while they stand within the home's reach and the host
/// is here: walked away, or the host gone, it goes.
#[test]
fn a_guest_loses_the_comfort_away_from_it_or_with_the_host_gone() {
    let mut server = fixture();
    let reach = server.survival.home_reach as f32;
    server.players.get_mut(&1).unwrap().position = Vec3::new(12.5 + reach + 2.0, (FLOOR + 1) as f32, 12.5);
    crate::process::Survival.run(&mut server);
    assert!(!hearthside(&server, 1), "Hearthside past the hearth's reach");
    server.players.get_mut(&1).unwrap().position = Vec3::new(10.5, (FLOOR + 1) as f32, 12.5);
    crate::process::Survival.run(&mut server);
    assert!(hearthside(&server, 1));
    server.players.get_mut(&2).unwrap().joined = false;
    crate::process::Survival.run(&mut server);
    assert!(!hearthside(&server, 1), "Hearthside at the hearth of a host who is not here");
}
