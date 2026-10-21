//! Fishing (`base:fish`, engine `fishing`): a line cast into water comes up, once its time
//! is up, with something that bites there for the angler's level and rod, or nothing.

use voxmar_engine::dropped;
use voxmar_engine::fishing::Spot;
use voxmar_engine::glam::{IVec3, Vec2};
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::lang::Words;
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage};

use crate::Server;
use crate::items::random3;

/// Rain (or snow) at least this heavy counts as rain for what bites.
const RAINING: f32 = 0.2;

/// The tags a catch reads where a line is cast: its biome's, and the water's climate from
/// its temperature, °C. The sea is one biome from pole to equator, and its fish want cold
/// water or warm.
fn spot_tags(biome: &[String], temperature: f32, rules: &voxmar_engine::fishing::FishingRules) -> Vec<String> {
    biome.iter().cloned().chain([rules.water.tag(temperature).to_owned()]).collect()
}

/// The draws of a player's `cast`th line (counting from 0) into `at`, by their draw key
/// (`Player::draw_key`, from the world's secret one). Numbered by the player's own casts,
/// which their save keeps, rather than by the tick the line lands on: a cast's luck does
/// not follow how the frames and messages fell, no count is drawn twice, a reload
/// included, and nothing a client knows (the seed, its id, the cell) tells it the draw.
fn cast_draws(key: u64, cast: u64, at: IVec3) -> [f32; 3] {
    random3(key ^ cast.wrapping_mul(0x9e37_79b9_7f4a_7c15), at, 0xf15)
}

impl Server {
    /// What the line `agent` cast into the water at `at` brings up.
    pub(crate) fn fish(&mut self, agent: PlayerId, at: IVec3) {
        let fishing = self.content.fishing.clone();
        let Some(player) = self.players.get_mut(&agent) else { return };
        let cast = player.casts;
        player.casts += 1;
        let player = &self.players[&agent];
        let level = fishing.skill.map_or(1, |skill| self.check_level(agent, skill));
        let rod = self.content.tools.tool(player.held());
        let site = self.overworld.site(at.x, at.z);
        let tags = spot_tags(&self.overworld.biomes().get(site.biome).tags, site.temperature, &fishing.rules);
        let (tick, day) = (self.tick, self.day);
        let raining = self.weather.local(&mut self.temperature, Vec2::new(at.x as f32, at.z as f32), tick, day).weather.precipitation >= RAINING;
        let spot = Spot {
            salt: tags.iter().any(|t| t == "ocean"),
            biome_tags: &tags,
            day: voxmar_engine::creature::daylight(self.day as u64) > 0.3,
            raining,
            level,
            rod_tier: rod.tier,
            rod_yield: rod.yield_,
            luck: player.modifiers.fishing,
        };
        let [land, pick, _] = cast_draws(player.draw_key, cast, at);
        let Some(catch) = fishing.cast(&spot, land, pick).cloned() else {
            if let Some(p) = self.players.get(&agent) {
                p.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("fishing.nothing") });
            }
            return;
        };
        let player = self.players.get_mut(&agent).expect("checked above");
        let left = player.inventory.add(catch.item, 1);
        player.changed(traits::INVENTORY);
        if left > 0 {
            // Where it pops is seen by all, so it draws nothing from the cast's draws.
            let body = dropped::popped(at + IVec3::Y, self.meta.next_pop(at + IVec3::Y, 0xf15));
            self.items.spawn(ItemStack::new(catch.item, left), body, dropped::PICKUP_DELAY);
        }
        if let Some(skill) = fishing.skill {
            self.grant_xp(agent, skill, catch.def.xp);
        }
        self.discover(agent, voxmar_engine::journal::Page::Fish, &catch.def.item, 1);
    }
}

#[cfg(test)]
mod tests {
    use voxmar_engine::fishing::Water;
    use voxmar_engine::glam::{IVec3, Vec3};

    use super::cast_draws;
    use crate::Server;

    /// The same world, water, player and count of casts land the same catches, whatever
    /// tick each cast lands on.
    #[test]
    fn a_cast_lands_by_its_count_not_its_tick() {
        let catches = |offset: u64| {
            let mut server = angler();
            server.tick += offset;
            (0..24)
                .map(|i| {
                    server.tick += 100 + i * offset % 37;
                    let before = held_fish(&server);
                    server.fish(1, WATER);
                    held_fish(&server).into_iter().find(|f| !before.contains(f))
                })
                .collect::<Vec<_>>()
        };
        let first = catches(0);
        assert!(first.iter().any(Option::is_some) && first.iter().any(Option::is_none), "some casts land and some miss: {first:?}");
        for offset in [1, 7, 12_345] {
            assert_eq!(catches(offset), first, "{offset} ticks later");
        }
    }

    /// Numbered draws keep the odds: a cast lands when its `land` draw is under the chance,
    /// so over many casts the share under each chance is that chance (within four standard
    /// errors), in any world, water and player; and the catch drawn does not lean on it.
    #[test]
    fn numbered_casts_land_at_their_odds() {
        const CASTS: u64 = 4000;
        for (key, at) in [(0x5eed, WATER), (0x9d2c_5680_a1b3_e7f1, IVec3::new(-3000, 62, 811)), (crate::player::player_draw_key(7, "Ünï"), IVec3::new(17, 120, -40))] {
            let draws: Vec<[f32; 3]> = (0..CASTS).map(|n| cast_draws(key, n, at)).collect();
            for chance in [0.35f32, 0.6, 0.9] {
                let share = draws.iter().filter(|d| d[0] < chance).count() as f32 / CASTS as f32;
                let within = 4.0 * (chance * (1.0 - chance) / CASTS as f32).sqrt();
                assert!((share - chance).abs() < within, "key {key:x} at {at}: {share} of casts under {chance}");
            }
            let landed: Vec<f32> = draws.iter().filter(|d| d[0] < 0.5).map(|d| d[1]).collect();
            let mean = landed.iter().sum::<f32>() / landed.len() as f32;
            let within = 4.0 * (1.0 / 12.0 / landed.len() as f32).sqrt();
            assert!((mean - 0.5).abs() < within, "key {key:x} at {at}: the catch drawn leans {mean} on landed casts");
        }
    }

    /// Two players casting the same count into the same water draw apart, and the same
    /// player in two worlds of one seed does too: the draw follows the world's secret key
    /// and the player's name, which no client knows together, not the seed or a connection.
    #[test]
    fn a_cast_draws_by_the_world_key_and_the_player() {
        let draws = |world: u64, name: &str| (0..64).map(|n| cast_draws(crate::player::player_draw_key(world, name), n, WATER)).collect::<Vec<_>>();
        assert_ne!(draws(1, "alex"), draws(1, "sam"), "two players");
        assert_ne!(draws(1, "alex"), draws(2, "alex"), "two worlds");
        assert_eq!(draws(1, "alex"), draws(1, "ALEX"), "one player, however they type their name");
    }

    const WATER: IVec3 = IVec3::new(130, 199, 128);

    /// Player 1 on a test world, holding a rod beside a pool.
    fn angler() -> Server {
        let mut server = crate::test_server();
        server.meta.draw_key = 0x0d1a;
        let water = server.content.blocks.expect("base:water");
        for dx in -2..=2 {
            for dz in -2..=2 {
                server.world.set_block(WATER + IVec3::new(dx, 0, dz), water);
            }
        }
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let mut player = crate::player::Player::new(1, connection, Vec3::new(128.5, 200.0, 128.5), 2, server.tick, server.survival.max_health, &server.config.limits);
        (player.joined, player.name) = (true, "Angler".into());
        player.draw_key = crate::player::player_draw_key(server.meta.draw_key, &player.name);
        let rod = server.content.blocks.expect("base:fishing_rod");
        assert_eq!(player.inventory.add(rod, 1), 0);
        player.selected = (0..voxmar_engine::inventory::SLOTS).find(|&s| player.inventory.get(s).is_some_and(|s| s.item == rod)).unwrap();
        server.players.insert(1, player);
        server
    }

    /// The fish in player 1's pack, one entry a fish.
    fn held_fish(server: &Server) -> Vec<voxmar_engine::block::BlockId> {
        let blocks = &server.content.blocks;
        (0..voxmar_engine::inventory::SLOTS)
            .filter_map(|s| server.players[&1].inventory.get(s))
            .filter(|s| blocks.def(s.item).has_tag("fish"))
            .flat_map(|s| std::iter::repeat_n(s.item, s.count as usize))
            .collect()
    }

    /// Every catch bites somewhere: in a biome with the water it wants (salt in the sea),
    /// with one of its biome tags among those a cast there reads at some temperature from
    /// −30 to 45 °C. Cod, tuna and swordfish once bit nowhere: the sea is one biome, tagged
    /// neither cold nor warm, until the water's temperature gave it a climate.
    #[test]
    fn every_catch_bites_somewhere() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = voxmar_engine::content::Content::load(&dir).unwrap();
        let world = voxmar_worldgen::WorldContent::load(&content).unwrap();
        let nowhere: Vec<&str> = content
            .fishing
            .iter()
            .filter(|catch| {
                let def = &catch.def;
                !world.biomes.iter().any(|biome| {
                    let salt = biome.tags.iter().any(|t| t == "ocean");
                    let water = match def.water {
                        Water::Any => true,
                        Water::Salt => salt,
                        Water::Fresh => !salt,
                    };
                    let tagged = |temperature: i32| {
                        let tags = super::spot_tags(&biome.tags, temperature as f32, &content.fishing.rules);
                        def.biomes.is_empty() || def.biomes.iter().any(|t| tags.contains(t))
                    };
                    water && (-30..=45).any(tagged)
                })
            })
            .map(|catch| catch.def.item.as_str())
            .collect();
        assert!(nowhere.is_empty(), "these never bite anywhere: {nowhere:?}");
    }
}
