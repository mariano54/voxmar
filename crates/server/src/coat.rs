//! Coating a block in place (blocks' `coat`): clay held to a placed wattle panel daubs it,
//! so a wattle home by the hearth is made sound and fireproof without being pulled down.
//! It asks the level of the recipe that makes what it becomes, and teaches that recipe's
//! experience a piece, so daubing in place is no cheaper a way to learn than the table.

use voxmar_engine::glam::IVec3;
use voxmar_engine::lang::Words;
use voxmar_protocol::PlayerId;

use crate::Server;
use voxmar_engine::traits;

impl Server {
    pub(crate) fn coat(&mut self, agent: PlayerId, pos: IVec3) {
        let Some(here) = self.block(pos) else { return };
        let Some((with, into)) = self.content.blocks.coating(here) else { return };
        let Some(player) = self.players.get(&agent) else { return };
        let selected = player.selected;
        if player.inventory.get(selected).is_none_or(|stack| stack.item != with) {
            return;
        }
        // The easiest recipe that makes it: its skill and level, and its experience a piece.
        let recipe = self.content.recipes.iter().filter(|r| r.output == into).min_by_key(|r| (r.level, r.skill.map(|s| s.0)));
        let taught = recipe.and_then(|r| r.skill.map(|skill| (skill, r.level, r.xp / r.count.max(1) as f32)));
        if let Some((skill, level, _)) = taught
            && self.check_level(agent, skill) < level
        {
            let name = Words::key(&voxmar_engine::lang::def_key("skill", &self.content.skills.get(skill).name));
            self.refuse(agent, Words::key("station.level").with("skill", name).with("level", Words::text(level)));
            return;
        }
        let player = self.players.get_mut(&agent).unwrap();
        player.inventory.take_one(selected);
        player.changed(traits::INVENTORY);
        self.set_block(pos, into);
        if let Some((skill, _, xp)) = taught
            && xp > 0.0
        {
            self.grant_xp(agent, skill, xp);
        }
    }
}

#[cfg(test)]
mod tests {
    use voxmar_engine::glam::{IVec3, Vec3};
    use voxmar_engine::inventory::ItemStack;
    use voxmar_engine::coords::ColumnPos;

    use crate::player::Player;

    /// Clay held to a placed wattle panel daubs it: one clay gone, the panel a daub block,
    /// Construction taught. Below the daub recipe's level it is refused and nothing changes;
    /// a panel smouldering by a torch, daubed, no longer waits to catch.
    #[test]
    fn clay_daubs_a_placed_wattle_panel_in_place() {
        let mut server = crate::test_server();
        let (_client, connection) = voxmar_protocol::local_pair();
        let mut player = Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        let at = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 10, 8);
        let block = |server: &crate::Server, name: &str| server.content.blocks.expect(name);
        let (stone, wattle, clay, daub, torch) = (block(&server, "base:stone"), block(&server, "base:wattle_panel"), block(&server, "base:clay"), block(&server, "base:daub"), block(&server, "base:torch"));
        server.set_block(at - IVec3::Y, stone);
        server.set_block(at + IVec3::X - IVec3::Y, stone);
        server.set_block(at, wattle);
        server.set_block(at + IVec3::X, torch);
        assert!(server.blazes.contains_key(&at), "the panel by the torch smoulders");
        server.players.get_mut(&1).unwrap().position = at.as_vec3() + Vec3::new(0.5, 0.0, -1.5);
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack { item: clay, count: 3 });
        let construction = server.content.skills.id("core:construction").expect("construction");
        let level = server.content.recipes.iter().filter(|r| r.output == daub).map(|r| r.level).min().expect("a daub recipe");
        assert!(level > 1, "the daub recipe asks a level");
        server.coat(1, at);
        assert_eq!(server.block(at), Some(wattle), "daubed below the recipe's level");
        assert_eq!(server.players[&1].inventory.get(0).unwrap().count, 3);
        let short = server.content.skills.xp_for(level).saturating_sub(server.players[&1].skills.xp(construction));
        server.players.get_mut(&1).unwrap().skills.grant(construction, short);
        let xp = server.players[&1].skills.xp(construction);
        server.coat(1, at);
        assert_eq!(server.block(at), Some(daub), "not daubed");
        assert_eq!(server.players[&1].inventory.get(0).unwrap().count, 2, "one clay a panel");
        assert!(server.players[&1].skills.xp(construction) > xp, "daubing taught nothing");
        assert!(!server.blazes.contains_key(&at), "daub does not burn");
        // Daub is not coated again, and clay to a block with no coat does nothing.
        server.coat(1, at);
        assert_eq!(server.players[&1].inventory.get(0).unwrap().count, 2);
    }

    /// An earth wall daubs too: clay to the side of a dirt block coats it, by the action a
    /// right click proposes; clay to its top is laid there as before, never a coat.
    #[test]
    fn clay_to_the_side_of_an_earth_wall_daubs_it_and_to_the_top_does_not() {
        use voxmar_engine::action::ActionTarget;
        use voxmar_engine::coords::Face;
        let mut server = crate::test_server();
        let (_client, connection) = voxmar_protocol::local_pair();
        let mut player = Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        let at = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 10, 8);
        let block = |server: &crate::Server, name: &str| server.content.blocks.expect(name);
        let (stone, dirt, clay, daub) = (block(&server, "base:stone"), block(&server, "base:dirt"), block(&server, "base:clay"), block(&server, "base:daub"));
        for x in -1..=1 {
            for z in -3..=1 {
                server.set_block(at + IVec3::new(x, -1, z), stone);
            }
        }
        server.set_block(at, dirt);
        let player = server.players.get_mut(&1).unwrap();
        player.position = at.as_vec3() + Vec3::new(0.5, 0.0, -1.5);
        player.inventory.slots[0] = Some(ItemStack { item: clay, count: 3 });
        player.selected = 0;
        let construction = server.content.skills.id("core:construction").expect("construction");
        let level = server.content.recipes.iter().filter(|r| r.output == daub).map(|r| r.level).min().expect("a daub recipe");
        let xp = server.content.skills.xp_for(level);
        server.players.get_mut(&1).unwrap().skills.grant(construction, xp);
        let coat = server.content.actions.expect("base:coat");
        server.propose(1, coat, ActionTarget::Block { block: at, face: Face::PosY }, std::time::Instant::now());
        assert_eq!(server.block(at), Some(dirt), "coated from the top");
        assert_eq!(server.players[&1].inventory.get(0).unwrap().count, 3);
        server.propose(1, coat, ActionTarget::Block { block: at, face: Face::NegZ }, std::time::Instant::now());
        assert_eq!(server.block(at), Some(daub), "the earth wall's face was not daubed");
        assert_eq!(server.players[&1].inventory.get(0).unwrap().count, 2, "one clay a block");
        // Broken, daub gives daub, never the clay back: coating is no clay-to-xp loop.
        let info = server.content.blocks.info(daub);
        assert_eq!((info.drops, info.drop_count), (daub, 1), "daub breaks to itself");
        assert!(!server.content.recipes.iter().any(|r| r.output == clay), "something makes clay back");
    }
}
