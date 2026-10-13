//! Recording what players discover in their journal (see `voxmar_engine::journal`).

use voxmar_engine::block::BlockId;
use voxmar_engine::creature::Kind;
use voxmar_engine::glam::Vec3;
use voxmar_engine::journal::Page;
use voxmar_protocol::{PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;

/// Eye height above the feet, for what a player can see.
const EYE: f32 = 1.6;

impl Server {
    /// `agent` found `name` on `page`, and did it `times` more (0 for only seeing it): the
    /// journal records it, and the player hears of anything that changed.
    pub(crate) fn discover(&mut self, agent: PlayerId, page: Page, name: &str, times: u32) {
        let rules = &self.content.journal;
        let tick = self.tick;
        let Some(player) = self.players.get_mut(&agent) else { return };
        let news = player.journal.record(&rules, page, name, times, tick).is_some();
        if (news || times > 0)
            && let Some(entry) = player.journal.get(page, name)
        {
            player.connection.send(ServerMessage::Journal { entries: vec![entry.clone()] });
        }
    }

    /// The journal page a creature kind is found on; people are on none.
    pub(crate) fn page_of(kind: Kind) -> Option<Page> {
        match kind {
            Kind::Animal => Some(Page::Animals),
            Kind::Monster | Kind::Spirit => Some(Page::Monsters),
            Kind::Fish => Some(Page::Fish),
            Kind::Villager => None,
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
        // A wild plant whose item merged is found as that item (the journal lists the item).
        let name = self.content.blocks.def(self.content.blocks.item_of(block)).name.clone();
        self.discover(agent, page, &name, 1);
    }

    /// Once a second: each player finds the biome they stand in and the creatures in sight.
    pub(crate) fn look_around(&mut self) {
        if self.tick % TICKS_PER_SECOND as u64 != 0 {
            return;
        }
        let mut found: Vec<(PlayerId, Page, String)> = Vec::new();
        let rules = &self.content.journal;
        let reach = [Page::Animals, Page::Monsters, Page::Fish].into_iter().filter_map(|p| rules.seen_within(p)).fold(0.0f32, f32::max);
        for player in self.players.values().filter(|p| p.joined && p.health.alive()) {
            let feet = player.position.floor().as_ivec3();
            found.push((player.id, Page::Biomes, self.overworld.biome_at(feet.x, feet.z).name.clone()));
            let eye = player.position + Vec3::Y * EYE;
            for creature in self.creatures.live.values() {
                // A cheap test first: nothing past the widest page's reach is looked at.
                if creature.body.position.distance(player.position) > reach + 1.0 {
                    continue;
                }
                let species = self.content.creatures.get(creature.species);
                let Some(page) = Self::page_of(species.def.kind) else { continue };
                let Some(within) = rules.seen_within(page) else { continue };
                let body = creature.body.position + Vec3::Y * (species.def.body.size.1 * 0.5);
                if body.distance(eye) <= within && self.in_plain_view(eye, body) {
                    found.push((player.id, page, species.def.name.clone()));
                }
            }
        }
        for (id, page, name) in found {
            self.discover(id, page, &name, 0);
        }
    }

    /// Whether nothing solid lies on the straight line from `from` to `to` (sampled every
    /// quarter block).
    pub(crate) fn in_plain_view(&self, from: Vec3, to: Vec3) -> bool {
        let steps = ((to - from).length() * 4.0).ceil().max(1.0) as u32;
        (1..steps).all(|i| {
            let at = from.lerp(to, i as f32 / steps as f32).floor().as_ivec3();
            self.block(at).is_none_or(|b| !self.content.blocks.info(b).solid)
        })
    }
}
