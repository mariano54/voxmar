//! Recording what players discover in their journal (see `voxmar_engine::journal`).

use voxmar_engine::block::BlockId;
use voxmar_engine::creature::Kind;
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::journal::Page;
use voxmar_engine::lang::Words;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

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
    pub(crate) fn page_of(def: &voxmar_engine::creature::CreatureDef) -> Option<Page> {
        if def.boss {
            return Some(Page::Bosses);
        }
        match def.kind {
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

    /// `agent` broke a sample of ore the world laid at `pos`: the first they break over each
    /// vein teaches its rule's `prospect` (tools.ron) and tells them what lies below. Kept
    /// as a tally by the vein's middle (`prospected:x,y,z`), so it survives a save.
    pub(crate) fn prospect(&mut self, agent: PlayerId, pos: IVec3, block: BlockId) {
        if !self.content.blocks.def(block).has_tag("ore_sample") {
            return;
        }
        let Some((skill, xp)) = self.content.tools.rule(block).and_then(|rule| Some((rule.skill?, rule.prospect))).filter(|&(_, xp)| xp > 0.0) else { return };
        let Some(vein) = self.overworld.vein_of_sample(pos, block) else { return };
        let key = format!("prospected:{},{},{}", vein.x, vein.y, vein.z);
        let Some(player) = self.players.get_mut(&agent) else { return };
        if player.tallies.contains_key(&key) {
            return;
        }
        player.tallies.insert(key, 1);
        let ore = self.content.blocks.def(self.content.blocks.info(block).drops).name.clone();
        player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("prospect.vein").with("item", Words::Block(ore)) });
        self.grant_xp(agent, skill, xp);
    }

    /// Once a second: each player finds the biome they stand in and the creatures in sight.
    pub(crate) fn look_around(&mut self) {
        if self.tick % TICKS_PER_SECOND as u64 != 0 {
            return;
        }
        let mut found: Vec<(PlayerId, Page, String)> = Vec::new();
        let rules = &self.content.journal;
        let reach = [Page::Animals, Page::Monsters, Page::Bosses, Page::Fish].into_iter().filter_map(|p| rules.seen_within(p)).fold(0.0f32, f32::max);
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
                let Some(page) = Self::page_of(&species.def) else { continue };
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

#[cfg(test)]
mod tests {
    use voxmar_engine::glam::IVec3;

    /// The first coal sample a player breaks over a seam teaches its rule's prospecting
    /// experience; breaking it there again teaches none, nor does a sample where no seam's
    /// sample lies. (One a player set down over its seam: `actions.rs`, through the place
    /// and mine actions.)
    #[test]
    fn the_first_sample_over_a_vein_teaches_prospecting() {
        let mut server = crate::test_server();
        let (_client, connection) = voxmar_protocol::local_pair();
        let mut player = crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        let coal = server.content.blocks.expect("base:coal_ore_sample");
        let prospect = server.content.tools.rule(coal).expect("a rule for coal samples").prospect;
        assert!(prospect > 0.0, "coal samples teach no prospecting");
        let (at, sample) = server.overworld.locate_ore(Some("coal_seam"), voxmar_engine::glam::IVec2::new(server.spawn.x as i32, server.spawn.z as i32), 1500).expect("a coal seam near spawn");
        assert_eq!(sample, coal);
        let mining = server.content.skills.id("core:mining").unwrap();
        let xp = |server: &crate::Server| server.players[&1].skills.xp(mining);
        let before = xp(&server);
        server.prospect(1, at, coal);
        assert_eq!(xp(&server) - before, (prospect * 10.0).round() as u32, "the first sample over the seam");
        let once = xp(&server);
        server.prospect(1, at, coal);
        assert_eq!(xp(&server), once, "the same seam taught twice");
        // Far from any seam's samples.
        let mut lone = at + IVec3::new(5000, 0, 5000);
        while server.overworld.vein_of_sample(lone, coal).is_some() {
            lone.x += 1;
        }
        server.prospect(1, lone, coal);
        assert_eq!(xp(&server), once, "a sample over no seam taught prospecting");
        assert!(server.players[&1].tallies.keys().filter(|k| k.starts_with("prospected:")).count() == 1);
    }
}
