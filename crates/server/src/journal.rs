//! Recording what players discover in their journal (see `voxmar_engine::journal`).

use voxmar_engine::block::BlockId;
use voxmar_engine::creature::Kind;
use voxmar_engine::journal::Page;
use voxmar_protocol::{PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;

/// How near a creature must come to be seen for the journal.
const SIGHT: f32 = 16.0;

impl Server {
    /// `agent` found `name` on `page`, and did it `times` more (0 for only seeing it): the
    /// journal records it, and the player hears of anything that changed.
    pub(crate) fn discover(&mut self, agent: PlayerId, page: Page, name: &str, times: u32) {
        let rules = self.content.journal.clone();
        let tick = self.tick;
        let Some(player) = self.players.get_mut(&agent) else { return };
        let news = player.journal.record(&rules, page, name, times, tick).is_some();
        if (news || times > 0)
            && let Some(entry) = player.journal.get(page, name)
        {
            player.connection.send(ServerMessage::Journal { entries: vec![entry.clone()] });
        }
    }

    /// The journal page a creature kind is found on.
    pub(crate) fn page_of(kind: Kind) -> Page {
        match kind {
            Kind::Animal => Page::Animals,
            Kind::Monster | Kind::Spirit => Page::Monsters,
            Kind::Fish => Page::Fish,
        }
    }

    /// `agent` broke `block`: an ore, gem or rock goes on the Ores page, a plant on Plants.
    pub(crate) fn discover_broken(&mut self, agent: PlayerId, block: BlockId) {
        // A crop's stages are found when it is harvested (see `harvest`).
        if self.content.crops.stage_of(block).is_some() {
            return;
        }
        let (def, info) = (self.content.blocks.def(block), self.content.blocks.info(block));
        let page = if def.has_tag("ore") || def.has_tag("gem") || (def.has_tag("stone") && def.has_tag("natural")) {
            Page::Ores
        } else if !info.solid && (def.has_tag("plant") || def.has_tag("flower") || def.has_tag("herb") || def.has_tag("fungus")) {
            Page::Plants
        } else {
            return;
        };
        let name = def.name.clone();
        self.discover(agent, page, &name, 1);
    }

    /// Once a second: each player finds the biome they stand in and the creatures in sight.
    pub(crate) fn look_around(&mut self) {
        if self.tick % TICKS_PER_SECOND as u64 != 0 {
            return;
        }
        let mut found: Vec<(PlayerId, Page, String)> = Vec::new();
        for player in self.players.values().filter(|p| p.joined && p.health.alive()) {
            let feet = player.position.floor().as_ivec3();
            found.push((player.id, Page::Biomes, self.overworld.biome_at(feet.x, feet.z).name.clone()));
            for creature in self.creatures.live.values() {
                if creature.body.position.distance(player.position) <= SIGHT {
                    let species = self.content.creatures.get(creature.species);
                    found.push((player.id, Self::page_of(species.def.kind), species.def.name.clone()));
                }
            }
        }
        for (id, page, name) in found {
            self.discover(id, page, &name, 0);
        }
    }
}
