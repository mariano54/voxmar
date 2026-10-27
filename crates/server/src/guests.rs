//! A guest at another player's lit hearth (docs/home.md "A guest at a friend's hearth"):
//! away from their own home, within reach of an online player's lit hearth, a player takes
//! that home's comfort, and sleeps there as well as it gives; its steps are never announced
//! to them nor counted for their quests.

use voxmar_engine::effect::HomeSurvey;
use voxmar_protocol::PlayerId;

use crate::Server;

impl Server {
    /// What lies around the home player `id` is a guest in, away from their own: another
    /// player's lit hearth (never their bed) they stand within reach of, that player here.
    /// Its comfort is the guest's while they stay, the host's animals counting for it; its
    /// steps are never announced to them nor counted for their quests, and its stores are
    /// not theirs to lay from (`home_stores` keeps to what the guest placed).
    pub(crate) fn guest_survey(&self, id: PlayerId) -> Option<HomeSurvey> {
        let at = self.players.get(&id)?.position;
        // The lowest-numbered host, should two homes' reaches overlap.
        let (host, anchor) = self
            .players
            .iter()
            .filter(|&(&other, p)| other != id && p.joined)
            .filter_map(|(&other, p)| {
                let anchor = p.hearth_spot(&self.lit)?.floor().as_ivec3();
                self.in_home_box(anchor, at).then_some((other, anchor))
            })
            .min_by_key(|&(other, _)| other)?;
        Some(self.survey_around(anchor, Some(host)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use voxmar_engine::action::ActionTarget;
    use voxmar_engine::block::BlockId;
    use voxmar_engine::coords::{ColumnPos, Face};
    use voxmar_engine::glam::{IVec3, Vec3};
    use voxmar_engine::world::Column;

    use crate::Server;
    use crate::player::Player;
    use crate::process::Process;

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
        server.lit.insert(HEARTH, crate::hearth::Lit { position: HEARTH.to_array(), structure: "base:homestead_hearth".into(), until: u64::MAX, owner: None });
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

    /// Home again: the host back by their lit hearth after ten minutes out is welcomed home
    /// for five (`homecomings`); after a short step out, nothing. The guest, with no home of
    /// their own, is never away from one, and comes to nobody's home.
    #[test]
    fn coming_home_after_a_long_day_out_is_a_welcome_home() {
        let mut server = fixture();
        let effect = server.content.effects.id("base:home_again").expect("base's home_again");
        let welcomed = |server: &Server, who: u32| server.players[&who].timed_effects.iter().find(|(e, _)| *e == effect).map(|&(_, left)| left);
        let (home, out) = (Vec3::new(13.5, (FLOOR + 1) as f32, 12.5), Vec3::new(13.5 + server.survival.home_reach as f32 + 3.0, (FLOOR + 1) as f32, 12.5));
        let outing = |server: &mut Server, seconds: f32| {
            server.players.get_mut(&2).unwrap().position = out;
            server.players.get_mut(&1).unwrap().position = out;
            // Each pass counts its seconds away, up to as many passes as a second a pass.
            for _ in 0..seconds.ceil() as usize {
                if server.players[&2].away >= seconds {
                    break;
                }
                crate::process::Survival.run(server);
            }
            assert!(server.players[&2].away >= seconds, "time away is not counted");
            server.players.get_mut(&2).unwrap().position = home;
            server.players.get_mut(&1).unwrap().position = Vec3::new(10.5, (FLOOR + 1) as f32, 12.5);
            crate::process::Survival.run(server);
        };
        crate::process::Survival.run(&mut server);
        assert_eq!(welcomed(&server, 2), None, "welcomed home without going out");
        outing(&mut server, 60.0);
        assert_eq!(welcomed(&server, 2), None, "a short step out is no homecoming");
        assert_eq!(server.players[&2].away, 0.0, "the time out is not kept once home");
        outing(&mut server, 600.0);
        assert!(welcomed(&server, 2).is_some_and(|left| left > 290.0), "not welcomed home after a long day out");
        assert!(server.players[&2].vitals.effects.iter().any(|a| a.effect == effect), "the welcome does not show");
        assert_eq!(server.players[&1].away, 0.0, "a player with no home counted time away");
        assert_eq!(welcomed(&server, 1), None, "a guest welcomed home to another's hearth");
    }

    /// By a friend's hearth, the guest is Hearthside as the host is; it is not announced to
    /// them, kept as reached or felt for their quests, so their own hearth does all that.
    #[test]
    fn a_guest_by_a_friends_hearth_is_hearthside_unannounced() {
        let mut server = fixture();
        // What the guest last saw of their own home, kept to tell them on their return what
        // it lost (`flames::check_home_return`).
        let theirs = (vec![server.content.effects.id("base:camp").unwrap()], Some(voxmar_engine::glam::IVec3::new(-40, 64, -40)));
        let guest = server.players.get_mut(&1).unwrap();
        (guest.home_steps, guest.home_anchor, guest.home_unchecked) = (theirs.0.clone(), theirs.1, false);
        crate::process::Survival.run(&mut server);
        assert_eq!((server.players[&1].home_steps.clone(), server.players[&1].home_anchor), theirs, "a visit wrote the host's home over the guest's own");
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

    /// A guest who sleeps in a bed by a friend's hearth, with a seat and two lights by it (the
    /// host's Bedside), wakes well rested as the host would, though their own hearth is far off;
    /// with the host gone, it is a plain rest.
    #[test]
    fn a_guest_sleeps_as_well_as_the_home_they_are_a_guest_in() {
        let rested = |host_here: bool| {
            let mut server = fixture();
            let (bag, stool, torches) = (HEARTH + IVec3::new(-3, 0, 1), HEARTH + IVec3::new(-3, 0, -1), [HEARTH + IVec3::new(-1, 0, 2), HEARTH + IVec3::new(-1, 0, -2)]);
            for (at, name) in [(bag, "base:sleeping_bag"), (stool, "base:oak_stool"), (torches[0], "base:torch"), (torches[1], "base:torch")] {
                let block = id(&server, name);
                server.set_block(at, block);
            }
            // The guest's own hearth burns far off, so it, not this bed, is their home.
            let far = IVec3::new(400, FLOOR + 1, 400);
            server.players.get_mut(&1).unwrap().home = Some(far.as_vec3() + Vec3::new(0.5, 1.0, 0.5));
            server.lit.insert(far, crate::hearth::Lit { position: far.to_array(), structure: "base:homestead_hearth".into(), until: u64::MAX, owner: None });
            server.players.get_mut(&2).unwrap().joined = host_here;
            // A night's sleep: the well rested one is a night's only.
            server.day = (voxmar_protocol::TICKS_PER_DAY as f64 * 0.9).round();
            assert!(server.night());
            let sleep = server.content.actions.expect("base:sleep");
            server.propose(1, sleep, ActionTarget::Block { block: bag, face: Face::PosY }, Instant::now());
            let effects = &server.content.effects;
            let well = effects.id("base:well_rested").expect("base's well_rested");
            let plain = effects.id("base:rested").expect("base's rested");
            let given = &server.players[&1].timed_effects;
            (given.iter().any(|(e, _)| *e == well), given.iter().any(|(e, _)| *e == plain))
        };
        assert_eq!(rested(true).0, true, "a guest by a friend's Bedside did not sleep well rested");
        assert_eq!(rested(false), (false, true), "with the host gone, the guest's sleep is a plain rest");
    }
}
