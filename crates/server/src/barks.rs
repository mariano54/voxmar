//! Barks: the short, wordless sounds settlements' people make as they react to something
//! (a greeting hum, a cry of alarm, a content sigh). The server only says who barked
//! what; each client plays it as that creature's voice gives it (`creature_sounds.ron`).

use voxmar_engine::creature::Bark;
use voxmar_protocol::{ServerMessage, TICKS_PER_SECOND};

use crate::Server;

/// Blocks within which `/creatures bark` finds one of a settlement's people to bark.
pub const BARK_COMMAND_REACH: f32 = 32.0;

impl Server {
    /// Creature `id` makes `bark`, heard by every player within its `barking.range` who
    /// sees it now, each no more than once in `barking.every` (per bark, per player). The
    /// one hook for what settlements' people say without words: whatever makes them react
    /// calls it.
    pub(crate) fn bark(&mut self, id: u32, bark: Bark) {
        self.bark_to(id, bark, false);
    }

    /// As [`Server::bark`]; `asked` (an admin's command) heeds neither range nor wait,
    /// heard by every player who sees them.
    fn bark_to(&mut self, id: u32, bark: Bark, asked: bool) {
        let Some(c) = self.creatures.live.get(&id) else { return };
        let barking = self.content.creatures.get(c.species).def.barking;
        let (at, tick, every) = (c.body.position, self.tick, (barking.every * TICKS_PER_SECOND as f32) as u64);
        let heard = &mut self.villagers.barked;
        heard.retain(|_, until| *until > tick);
        for player in self.players.values().filter(|p| p.joined && (asked || p.position.distance(at) <= barking.range) && self.creatures.sees(p.id, id)) {
            let until = heard.entry((id, player.id, bark)).or_insert(0);
            if asked || *until <= tick {
                *until = tick + every;
                player.connection.send(ServerMessage::Bark { creature: id, bark });
            }
        }
    }

    /// `/creatures bark NAME`: the nearest of a settlement's people within reach of `at`
    /// makes the bark called `name`. Whether one did, or `None` for a name that is no bark.
    pub(crate) fn bark_command(&mut self, at: voxmar_engine::glam::Vec3, name: &str) -> Option<usize> {
        let bark = Bark::ALL.into_iter().find(|b| b.name() == name)?;
        let villager = |species| self.content.creatures.get(species).def.kind == voxmar_engine::creature::Kind::Villager;
        let nearest = self
            .creatures
            .live
            .iter()
            .filter(|(_, c)| villager(c.species) && c.body.position.distance(at) <= BARK_COMMAND_REACH)
            .min_by(|a, b| a.1.body.position.distance(at).total_cmp(&b.1.body.position.distance(at)))
            .map(|(&id, _)| id);
        if let Some(id) = nearest {
            self.bark_to(id, bark, true);
        }
        Some(nearest.is_some() as usize)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use voxmar_engine::coords::ColumnPos;
    use voxmar_engine::creature::Bark;
    use voxmar_engine::glam::{IVec3, Vec3};
    use voxmar_engine::world::Column;
    use voxmar_protocol::ServerMessage;

    /// A bark is told to the players who see the creature, and none is told of one that
    /// is not there; `/creatures bark` makes the nearest of a settlement's people bark, and
    /// says so, or that no one was near.
    #[test]
    fn a_bark_reaches_those_who_see_the_barker() {
        let mut server = crate::test_server();
        server.world = Default::default();
        let stone = server.content.blocks.expect("base:stone");
        let mut column = Column::default();
        for x in 0..32 {
            for z in 0..32 {
                column.set_block(IVec3::new(x, 64, z), stone);
            }
        }
        server.world.insert(ColumnPos::new(0, 0), Arc::new(column));
        let (client, connection) = voxmar_protocol::local_pair();
        let mut player = crate::player::Player::new(1, connection, Vec3::new(4.5, 65.0, 4.5), 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        let species = server.content.creatures.id("base:villager").expect("a villager species");
        let id = server.place_creature(species, Vec3::new(8.5, 65.0, 4.5), 7, 0.0).expect("placed");
        server.tick();
        let barks = |client: &voxmar_protocol::ClientConnection| {
            std::iter::from_fn(|| client.try_recv().ok().flatten())
                .filter_map(|m| if let ServerMessage::Bark { creature, bark } = m { Some((creature, bark)) } else { None })
                .collect::<Vec<_>>()
        };
        barks(&client);
        server.bark(id, Bark::Greet);
        assert_eq!(barks(&client), [(id, Bark::Greet)]);
        server.bark(id + 1000, Bark::Alarm);
        assert_eq!(barks(&client), []);
        assert_eq!(server.bark_command(Vec3::new(4.5, 65.0, 4.5), "content"), Some(1));
        assert_eq!(barks(&client), [(id, Bark::Content)]);
        assert_eq!(server.bark_command(Vec3::new(4.5, 65.0, 4.5), "sing"), None);
        // The command says how far it looked, and that no one barked with no one near.
        let said = |client: &voxmar_protocol::ClientConnection| {
            std::iter::from_fn(|| client.try_recv().ok().flatten())
                .filter_map(|m| if let ServerMessage::Chat { text: voxmar_engine::lang::Words::Key(key, _), .. } = m { Some(key) } else { None })
                .collect::<Vec<_>>()
        };
        server.players.get_mut(&1).unwrap().admin = true;
        server.command(1, "creatures bark greet");
        assert_eq!(said(&client), ["command.creatures.barked"]);
        server.creatures.take_away(id);
        server.command(1, "creatures bark greet");
        assert_eq!(said(&client), ["command.creatures.no_one_to_bark"]);
    }
}
