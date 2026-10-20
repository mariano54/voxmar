//! Tools and weapons wearing out in the hand (`voxmar_engine::tool::Wear`, each package's
//! `tools.ron`): a block broken, soil tilled or a blow that lands takes uses from what the
//! player holds, the skill its kind trains saves one now and then, and worn to nothing it
//! breaks (leaving nothing, or a head a pack names). The player hears it warn once it is nearly worn
//! out, and hears and sees it break (`ServerMessage::ToolWore`). Creative play wears nothing.

use voxmar_engine::inventory::{CURSOR, ItemStack};
use voxmar_engine::tool::{Worn, wear_down};
use voxmar_protocol::{PlayerId, ServerMessage};

use crate::Server;
use crate::save::Draw;

impl Server {
    /// Wears what `agent` holds by `uses` (it says how many a use takes).
    pub(crate) fn wear_held(&mut self, agent: PlayerId, uses: u16) {
        let Some(slot) = self.players.get(&agent).map(|p| p.selected) else { return };
        self.wear_slot(agent, slot, uses);
    }

    /// Wears what lies in `agent`'s `slot` (a pack slot, or the pointer, `CURSOR`) by
    /// `uses`: the one that did the work, such as the weapon a swing began with.
    pub(crate) fn wear_slot(&mut self, agent: PlayerId, slot: usize, uses: u16) {
        let (tools, blocks, skills) = (self.content.tools.clone(), self.content.blocks.clone(), self.content.skills.clone());
        let Some(player) = self.players.get_mut(&agent) else { return };
        let Some(mut stack) = player.inventory.get(slot) else { return };
        let total = tools.uses(stack.item);
        if total == 0 || uses == 0 || player.mode() == crate::save::GameMode::Creative {
            return;
        }
        // By the player's own count of uses that might have worn, never the tick.
        let wielded = self.content.combat.weapon(stack.item).and_then(|weapon| weapon.skill);
        if let Some(skill) = tools.wear_skill(stack.item, blocks.def(stack.item).has_tag("weapon"), wielded) {
            let level = skills.level(player.skills.xp(skill));
            let [draw, _] = player.draw(Draw::Wear);
            if tools.saves_wear(level, draw) {
                return;
            }
        }
        let item = stack.item;
        // Only one wears: were there ever more than one in the stack, the rest stay as
        // they were, apart from it (in the pack, or at the feet with no room, theirs and
        // lying as long as a grave).
        let rest = (stack.count > 1).then(|| ItemStack { count: stack.count - 1, ..stack });
        stack.count = 1;
        let worn = wear_down(&mut stack, uses, total, tools.wear().warn);
        let now = match worn {
            Worn::Broke => tools.head(item).map(|head| ItemStack::new(head, 1)),
            _ => Some(stack),
        };
        match slot {
            CURSOR => player.inventory.cursor = now,
            _ => player.inventory.slots[slot] = now,
        }
        let mut left = Vec::new();
        for piece in rest.into_iter().flat_map(voxmar_engine::inventory::split_stack) {
            let over = player.inventory.add_stack(piece);
            if over > 0 {
                left.push(ItemStack { count: over, ..piece });
            }
        }
        player.changed(voxmar_engine::traits::INVENTORY);
        if worn != Worn::Fine {
            player.connection.send(ServerMessage::ToolWore { item, broke: worn == Worn::Broke });
        }
        let (feet, owner) = (player.position.floor().as_ivec3(), crate::items::owner_of(&player.name));
        let lifetime = (self.content.value.death.grave * voxmar_protocol::TICKS_PER_SECOND as f32) as u32;
        for (i, stack) in left.into_iter().enumerate() {
            let random = self.meta.next_pop(feet, 0x3ea7 ^ i as u32);
            let body = voxmar_engine::dropped::popped(feet, random);
            self.items.spawn_for(stack, body, voxmar_engine::dropped::PICKUP_DELAY, Some(owner), lifetime);
        }
    }

    /// Uses breaking or tilling the block at `pos` takes from the tool it was done with:
    /// `tool` is the hotbar slot in hand, what was in it and how worn (`Timed::tool`). One
    /// switched to since is not worn. Moved out of that slot since, the wear goes to the
    /// same copy wherever it is carried (a pack slot or the pointer, found by its wear), else
    /// the first carrying that tool, so moving it never dodges the wear (several breaks
    /// shown done early can be waiting, and each wears it); with none carried, nothing wears.
    pub(crate) fn wear_on_block(&mut self, agent: PlayerId, pos: voxmar_engine::glam::IVec3, tilling: bool, (slot, began, worn): crate::actions::Tool) {
        let tools = &self.content.tools;
        let Some(item) = began else { return };
        let Some(player) = self.players.get(&agent) else { return };
        let carries = |spot: usize, wear: Option<u16>| player.inventory.get(spot).is_some_and(|s| s.item == item && wear.is_none_or(|w| s.wear == w));
        let elsewhere = |wear: Option<u16>| (0..player.inventory.slots.len()).chain([CURSOR]).find(|&s| carries(s, wear));
        let Some(slot) = carries(slot, None).then_some(slot).or_else(|| elsewhere(Some(worn))).or_else(|| elsewhere(None)) else { return };
        let uses = if tilling {
            if tools.uses(item) > 0 { tools.wear().till } else { 0 }
        } else {
            let Some(block) = self.block(pos) else { return };
            tools.break_wear(began, block, self.content.blocks.info(block).hardness)
        };
        self.wear_slot(agent, slot, uses);
    }
}

/// What burns while carried (a firebrand, `burns:N`) burns down a second at a time,
/// wherever it is in the inventory, counted in its stack's `wear`; burnt out it becomes
/// what it leaves (`burns_to:`), or nothing. Once a second.
#[derive(Default)]
pub(crate) struct Burning {
    /// Seconds gone by not yet burnt, under one.
    behind: f32,
}

impl crate::process::Process for Burning {
    fn tier(&self) -> voxmar_engine::schedule::Tier {
        voxmar_engine::schedule::Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
        self.behind += voxmar_engine::schedule::Tier::Low.period() as f32 / voxmar_protocol::TICKS_PER_SECOND as f32;
        let seconds = self.behind.floor();
        self.behind -= seconds;
        if seconds >= 1.0 {
            server.burn_carried(seconds as u16);
        }
    }
}

impl Server {
    /// Burns everything every living player carries that burns by `seconds`.
    pub(crate) fn burn_carried(&mut self, seconds: u16) {
        let tools = self.content.tools.clone();
        for player in self.players.values_mut().filter(|p| p.joined && p.health.alive()) {
            let inventory = &mut player.inventory;
            let mut burnt = false;
            for slot in inventory.slots.iter_mut().chain(std::iter::once(&mut inventory.cursor)).chain(inventory.craft.iter_mut()) {
                let Some(stack) = slot.as_mut() else { continue };
                let lasts = tools.burns(stack.item);
                if lasts == 0 {
                    continue;
                }
                burnt = true;
                stack.wear = stack.wear.saturating_add(seconds);
                if stack.wear >= lasts {
                    *slot = tools.burnt(stack.item).map(|left| ItemStack::new(left, 1));
                }
            }
            if burnt {
                player.changed(voxmar_engine::traits::INVENTORY);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use voxmar_engine::glam::IVec3;
    use voxmar_engine::inventory::ItemStack;
    use voxmar_protocol::{ClientConnection, ServerMessage};

    use crate::Server;

    /// A server with one joined survival player holding `item` (worn by `wear`) in slot 0,
    /// and the client end of their connection.
    fn holding(item: &str, wear: u16) -> (Server, ClientConnection) {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        let mut player = crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        (player.joined, player.allow_flight, player.draw_key) = (true, false, crate::player::player_draw_key(0x0d1a, "ada"));
        let item = server.content.blocks.expect(item);
        player.inventory.slots[0] = Some(ItemStack { wear, ..ItemStack::new(item, 1) });
        server.players.insert(1, player);
        (server, client)
    }

    /// What the player was told of their tool: (warned, broke).
    fn told(client: &ClientConnection) -> (u32, u32) {
        let mut told = (0, 0);
        while let Ok(Some(message)) = client.try_recv() {
            if let ServerMessage::ToolWore { broke, .. } = message {
                if broke { told.1 += 1 } else { told.0 += 1 }
            }
        }
        told
    }

    fn put(server: &mut Server, block: &str) -> IVec3 {
        let pos = server.spawn.floor().as_ivec3() + IVec3::new(2, 0, 0);
        let block = server.content.blocks.expect(block);
        server.set_block(pos, block);
        pos
    }

    /// A pickaxe wears a use for each stone it breaks, two for a block it is not made for
    /// (planks), none for one that breaks at a touch; it warns once at a tenth left, and worn
    /// out it breaks to nothing (or a head a pack names) and the player hears it.
    #[test]
    fn a_tool_wears_by_use_and_breaks() {
        let (mut server, client) = holding("base:flint_pickaxe", 0);
        let uses = server.content.tools.uses(server.content.blocks.expect("base:flint_pickaxe"));
        assert!(uses >= 10, "a flint pickaxe lasts {uses} uses");
        let wear = |server: &Server| server.players[&1].inventory.slots[0].map(|s| s.wear);
        let stone = put(&mut server, "base:stone");
        server.wear_on_block(1, stone, false, server.tool_in_hand(1));
        assert_eq!(wear(&server), Some(1), "a stone");
        let planks = put(&mut server, "base:oak_planks");
        server.wear_on_block(1, planks, false, server.tool_in_hand(1));
        assert_eq!(wear(&server), Some(3), "planks, which a pickaxe is not made for");
        let grass = put(&mut server, "base:short_grass");
        assert_eq!(server.content.blocks.info(server.content.blocks.expect("base:short_grass")).hardness, 0.0);
        server.wear_on_block(1, grass, false, server.tool_in_hand(1));
        assert_eq!(wear(&server), Some(3), "grass breaks at a touch");
        assert_eq!(told(&client), (0, 0));
        // Down to a tenth left, it warns once.
        let warn_at = (uses as f32 * server.content.tools.wear().warn).ceil() as u16;
        server.players.get_mut(&1).unwrap().inventory.slots[0].as_mut().unwrap().wear = uses - warn_at - 1;
        let stone = put(&mut server, "base:stone");
        server.wear_on_block(1, stone, false, server.tool_in_hand(1));
        server.wear_on_block(1, stone, false, server.tool_in_hand(1));
        assert_eq!(told(&client), (1, 0), "warned once, crossing a tenth left");
        server.players.get_mut(&1).unwrap().inventory.slots[0].as_mut().unwrap().wear = uses - 1;
        server.wear_on_block(1, stone, false, server.tool_in_hand(1));
        let pick = server.content.blocks.expect("base:stone_pickaxe");
        let left = server.content.tools.head(pick).map(|head| ItemStack::new(head, 1));
        assert_eq!(server.players[&1].inventory.slots[0], left, "broken, nothing but a head it names is left in the hand");
        assert_eq!(told(&client), (0, 1));
    }

    /// Mining saves a pickaxe's wear by the level, `level` times `save_per_level` a use
    /// (about one use in two at 99), drawn by the player's count of uses, never the tick.
    #[test]
    fn skill_saves_wear_by_the_count_of_uses_not_the_tick() {
        let expected = |level: u8| {
            let (server, _client) = holding("base:iron_pickaxe", 0);
            let key = server.players[&1].draw_key;
            (0..200).filter(|&n| !server.content.tools.saves_wear(level, crate::save::numbered_draws(key, crate::save::Draw::Wear.salt(), n)[0])).count() as u16
        };
        let worn = |tick: u64, level: u8| {
            let (mut server, _client) = holding("base:iron_pickaxe", 0);
            let mining = server.content.skills.id("core:mining").unwrap();
            let xp = server.content.skills.xp_for(level);
            server.players.get_mut(&1).unwrap().skills.grant(mining, xp);
            server.tick = tick;
            let stone = put(&mut server, "base:stone");
            for _ in 0..200 {
                server.wear_on_block(1, stone, false, server.tool_in_hand(1));
            }
            (server.players[&1].inventory.slots[0].unwrap().wear, server.players[&1].draws.wears)
        };
        assert_eq!(worn(1_000, 1).0, expected(1), "level 1 saves one use in 200");
        let (wear, draws) = worn(1_000, 99);
        assert_eq!(draws, 200, "each use counted on");
        assert_eq!(wear, expected(99), "each draw by its count");
        assert!((70..130).contains(&wear), "at 99 about half the uses wear: {wear} of 200");
        assert_eq!(worn(1_377, 99), (wear, draws), "the same uses later wore otherwise");
    }

    /// Were a stack of tools ever in hand, one wears and the rest stay as they were, apart
    /// from it: in the pack, or with no room at the feet, theirs and lying as long as a grave.
    #[test]
    fn only_one_of_a_stack_wears() {
        let (mut server, _client) = holding("base:iron_pickaxe", 4);
        let pick = server.content.blocks.expect("base:iron_pickaxe");
        server.players.get_mut(&1).unwrap().inventory.slots[0].as_mut().unwrap().count = 3;
        // Skill may save a use now and then; enough uses that some wear.
        server.wear_held(1, 20);
        let slots = server.players[&1].inventory.slots.clone();
        let held = slots[0].expect("still in hand");
        assert_eq!((held.item, held.count), (pick, 1));
        assert!(held.wear > 4, "the one in hand wore: {}", held.wear);
        let rest: Vec<ItemStack> = slots.iter().skip(1).flatten().copied().collect();
        assert_eq!(rest, [ItemStack { wear: 4, ..ItemStack::new(pick, 1) }; 2], "the rest kept their wear");
        // A full pack: the rest fall at the feet.
        let dirt = server.content.blocks.expect("base:dirt");
        let player = server.players.get_mut(&1).unwrap();
        for slot in player.inventory.slots.iter_mut().skip(1) {
            *slot = Some(ItemStack::new(dirt, 1));
        }
        player.inventory.slots[0] = Some(ItemStack { wear: 4, ..ItemStack::new(pick, 2) });
        let owner = crate::items::owner_of(&player.name);
        server.wear_held(1, 20);
        // A save writes a lifetime only where it is not the usual one.
        let grave = Some(server.content.value.death.grave).filter(|&g| g != voxmar_engine::dropped::LIFETIME);
        let lying: Vec<_> = server.items.saved(|item| server.content.blocks.def(item).name.clone()).into_iter().map(|i| (i.item, i.count, i.wear, i.owner, i.lifetime)).collect();
        assert_eq!(lying, [("base:iron_pickaxe".to_string(), 1, 4, Some(owner), grave)]);
    }

    /// Mining wears the tool it was done with (`Timed::tool`), from its slot: not one
    /// switched to before the block broke. Moved to another slot or the pointer it wears
    /// there, the same copy before another; no longer carried, nothing wears.
    #[test]
    fn mining_wears_the_tool_it_was_done_with() {
        let (mut server, _client) = holding("base:iron_pickaxe", 0);
        let pick = server.content.blocks.expect("base:iron_pickaxe");
        let began = server.tool_in_hand(1);
        assert_eq!(began, (0, Some(pick), 0));
        let player = server.players.get_mut(&1).unwrap();
        player.inventory.slots[1] = Some(ItemStack::new(pick, 1));
        player.selected = 1;
        let wear = |server: &Server, slot: usize| server.players[&1].inventory.slots[slot].map(|s| s.wear);
        // Enough stones that skill cannot save every use.
        for _ in 0..5 {
            let stone = put(&mut server, "base:stone");
            server.wear_on_block(1, stone, false, began);
        }
        assert!(wear(&server, 0).is_some_and(|w| w > 0), "the one it began with: {:?}", wear(&server, 0));
        assert_eq!(wear(&server, 1), Some(0), "the one switched to");
        let player = server.players.get_mut(&1).unwrap();
        player.inventory.slots[1] = None;
        player.inventory.slots[7] = player.inventory.slots[0].take();
        let before = wear(&server, 7);
        for _ in 0..5 {
            let stone = put(&mut server, "base:stone");
            server.wear_on_block(1, stone, false, began);
        }
        assert!(wear(&server, 7) > before, "moved to slot 8, it wore there: {before:?} to {:?}", wear(&server, 7));
        // On the pointer, the same copy is found by its wear, before a fresh one ahead of it.
        let player = server.players.get_mut(&1).unwrap();
        player.inventory.slots[2] = Some(ItemStack::new(pick, 1));
        player.inventory.cursor = player.inventory.slots[7].take();
        let cursor = |server: &Server| server.players[&1].inventory.cursor.map(|s| s.wear);
        let before = cursor(&server);
        for _ in 0..5 {
            let stone = put(&mut server, "base:stone");
            server.wear_on_block(1, stone, false, (0, Some(pick), cursor(&server).unwrap()));
        }
        assert!(cursor(&server) > before && wear(&server, 2) == Some(0), "on the pointer it wore, not the fresh one: {before:?} to {:?}, {:?}", cursor(&server), wear(&server, 2));
        let player = server.players.get_mut(&1).unwrap();
        player.inventory.slots[2] = None;
        player.inventory.slots[7] = player.inventory.cursor.take();
        let gone = server.players.get_mut(&1).unwrap().inventory.slots[7].take();
        let stone = put(&mut server, "base:stone");
        server.wear_on_block(1, stone, false, began);
        assert!(server.players[&1].inventory.slots.iter().all(Option::is_none), "no longer carried, nothing wears or comes back: {gone:?}");
    }

    /// `/wear` sets what is left of the tool in hand, or of one in a hotbar slot it names
    /// (from 1), whichever slot is in hand.
    #[test]
    fn wear_names_a_slot_or_takes_the_hand() {
        let (mut server, _client) = holding("base:iron_pickaxe", 0);
        let pick = server.content.blocks.expect("base:iron_pickaxe");
        let uses = server.content.tools.uses(pick);
        server.players.get_mut(&1).unwrap().inventory.slots[3] = Some(ItemStack::new(pick, 1));
        let me = crate::commands::Source::Player(1);
        let wear = |server: &Server, slot: usize| server.players[&1].inventory.slots[slot].map(|s| s.wear);
        assert!(server.wear_command(me, &["5".into()]).is_ok());
        assert_eq!((wear(&server, 0), wear(&server, 3)), (Some(uses - 5), Some(0)), "the hand");
        assert!(server.wear_command(me, &["2".into(), "4".into()]).is_ok());
        assert_eq!((wear(&server, 0), wear(&server, 3)), (Some(uses - 5), Some(uses - 2)), "slot 4");
        for bad in [["2", "0"], ["2", "10"], ["2", "x"]] {
            assert!(server.wear_command(me, &bad.map(String::from)).is_err(), "{bad:?}");
        }
        assert!(server.wear_command(me, &["2".into(), "2".into()]).is_err(), "an empty slot");
    }

    /// A hoe wears as it tills; in creative nothing wears; a block a player holds that never
    /// wears (a stick) and a bare hand are untouched.
    #[test]
    fn tilling_wears_and_creative_does_not() {
        let (mut server, _client) = holding("base:stone_hoe", 0);
        let soil = put(&mut server, "base:dirt");
        server.wear_on_block(1, soil, true, server.tool_in_hand(1));
        assert_eq!(server.players[&1].inventory.slots[0].unwrap().wear, server.content.tools.wear().till);
        server.players.get_mut(&1).unwrap().allow_flight = true;
        server.wear_on_block(1, soil, true, server.tool_in_hand(1));
        assert_eq!(server.players[&1].inventory.slots[0].unwrap().wear, server.content.tools.wear().till, "creative");
        let (mut server, _client) = holding("base:stick", 0);
        let stone = put(&mut server, "base:stone");
        server.wear_on_block(1, stone, false, server.tool_in_hand(1));
        assert_eq!(server.players[&1].inventory.slots[0].unwrap().wear, 0, "a stick never wears");
        server.players.get_mut(&1).unwrap().inventory.slots[0] = None;
        server.wear_on_block(1, stone, false, server.tool_in_hand(1));
        assert_eq!(server.players[&1].inventory.slots[0], None);
    }

    /// A worn tool keeps its wear when dropped, saved with the world's items and picked up
    /// again, and when it is kept through a death (the least worn of two the same).
    #[test]
    fn a_dropped_tool_keeps_its_wear() {
        let (server, _client) = holding("base:stone_axe", 37);
        let axe = server.players[&1].inventory.slots[0].unwrap();
        let mut items = crate::items::Items::default();
        items.spawn(axe, voxmar_engine::dropped::Loose::new(server.spawn, voxmar_engine::glam::Vec3::ZERO), 0.0);
        let blocks = server.content.blocks.clone();
        let saved = items.saved(|item| blocks.def(item).name.clone());
        assert_eq!(saved[0].wear, 37);
        let restored = crate::items::Items::restore(saved, |name| blocks.id(name));
        let again = restored.saved(|item| blocks.def(item).name.clone());
        assert_eq!((again[0].item.as_str(), again[0].wear), ("base:stone_axe", 37), "restored as worn");
        let stack = axe;
        let mut inventory = voxmar_engine::inventory::Inventory::default();
        inventory.add(axe.item, 1);
        assert_eq!(inventory.add_stack(stack), 0);
        assert_eq!(inventory.slots[1], Some(axe), "a worn one takes a slot of its own");
        // Through a death: the stone axes are worth the most, and the fresher is kept.
        let (kept, left) = crate::death::split(&inventory, 1, false, |item| u32::from(item == axe.item));
        assert_eq!(kept.slots[0], Some(ItemStack::new(axe.item, 1)), "the unworn one kept");
        assert_eq!(left, [axe], "the worn one left as worn");
        let mut worn_only = voxmar_engine::inventory::Inventory::default();
        worn_only.slots[5] = Some(axe);
        let (kept, _) = crate::death::split(&worn_only, 1, false, |_| 1);
        assert_eq!(kept.slots[0], Some(axe), "kept as worn as it was");
    }

    /// A worn tool put into a hopper by hand comes back as worn: a machine is no free
    /// mend (the factory's own tests follow it down a chute).
    #[test]
    fn a_hopper_keeps_a_tools_wear() {
        let (mut server, _client) = holding("base:iron_pickaxe", 300);
        let pick = server.players[&1].inventory.slots[0].unwrap();
        let at = put(&mut server, "base:hopper");
        let hopper = server.content.blocks.expect("base:hopper");
        let machines = server.content.machines.clone();
        server.factory.insert(at, hopper, &machines);
        server.use_machine(1, at);
        assert_eq!(server.players[&1].inventory.slots[0], None, "put in");
        assert_eq!(server.factory.get(at).unwrap().1.bin.iter().flatten().next(), Some(&pick), "worn in the hopper");
        server.use_machine(1, at);
        assert_eq!(server.players[&1].inventory.slots[0], Some(pick), "taken back as worn");
    }

    /// Saves from before tools wore held them in stacks: loaded, a stack of three pickaxes
    /// comes apart into three, in a player's slots (any with no room fall at their feet),
    /// a chest's bin and the items lying in the world; a worn tool in a chest stays worn.
    #[test]
    fn old_stacks_of_tools_come_apart_on_load() {
        let (mut server, _client) = holding("base:iron_pickaxe", 0);
        let pick = server.content.blocks.expect("base:iron_pickaxe");
        let blocks = server.content.blocks.clone();
        let dir = std::env::temp_dir().join(format!("voxmar-old-stacks-{}", std::process::id()));
        let save = crate::save::WorldSave::new(&dir);
        let mut player = server.players[&1].saved(&server.content.skills, &server.content.effects);
        player.inventory = voxmar_engine::inventory::Inventory::default();
        player.inventory.slots[0] = Some(ItemStack::new(pick, 3));
        save.write_player("ada", &player, &blocks).unwrap();
        let read = save.read_player("ada", &blocks).unwrap().unwrap();
        let picks: Vec<ItemStack> = read.inventory.slots.iter().flatten().copied().collect();
        assert_eq!(picks, [ItemStack::new(pick, 1); 3]);
        assert!(read.spilled.is_empty());
        // A full pack: what has no slot is spilled, to fall at their feet on joining.
        let dirt = blocks.expect("base:dirt");
        for slot in player.inventory.slots.iter_mut().skip(1) {
            *slot = Some(ItemStack::new(dirt, 1));
        }
        save.write_player("bo", &player, &blocks).unwrap();
        let read = save.read_player("bo", &blocks).unwrap().unwrap();
        assert_eq!((read.inventory.slots[0], read.spilled), (Some(ItemStack::new(pick, 1)), vec![ItemStack::new(pick, 1); 2]));
        // Joining, those fall at their feet: theirs alone, and lying as long as a grave.
        server.save = Some(save.clone());
        let (_bo, connection) = voxmar_protocol::local_pair();
        let joined = crate::player::Player::new(2, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        server.players.insert(2, joined);
        server.hello(2, "bo".into(), voxmar_protocol::PROTOCOL_VERSION, server.content.fingerprint(), None);
        // A save writes a lifetime only where it is not the usual one.
        let grave = Some(server.content.value.death.grave).filter(|&g| g != voxmar_engine::dropped::LIFETIME);
        let lying: Vec<_> = server.items.saved(|item| blocks.def(item).name.clone()).into_iter().map(|i| (i.item, i.count, i.owner, i.lifetime)).collect();
        assert_eq!(lying, vec![("base:iron_pickaxe".to_string(), 1, Some(crate::items::owner_of("bo")), grave); 2]);
        server.items = crate::items::Items::default();
        let _ = std::fs::remove_dir_all(&dir);
        // A chest's bin, as saved before and as saved worn.
        let at = put(&mut server, "base:chest");
        let chest = blocks.expect("base:chest");
        let machines = server.content.machines.clone();
        server.factory.insert(at, chest, &machines);
        server.factory.put(at, ItemStack { wear: 41, ..ItemStack::new(pick, 1) }, &machines);
        server.factory.put(at, ItemStack::new(pick, 1), &machines);
        let mut saved: Vec<_> = server.saved_machines().into_iter().filter(|m| m.position == at.to_array()).collect();
        let mut again = crate::test_server();
        again.set_block(at, chest);
        again.restore_machines(saved.clone());
        let bin: Vec<ItemStack> = again.factory.get(at).unwrap().1.bin.iter().flatten().copied().collect();
        assert_eq!(bin, [ItemStack { wear: 41, ..ItemStack::new(pick, 1) }, ItemStack::new(pick, 1)], "worn kept through a save");
        // A worn stack of several (half-burnt torches) keeps its wear too.
        let torch = blocks.expect("base:torch");
        let mut torches = saved.clone();
        torches[0].bin = vec![("base:torch".into(), 5), ("base:torch".into(), 2)];
        torches[0].bin_wear = vec![(1, 600)];
        let mut lit = crate::test_server();
        lit.set_block(at, chest);
        lit.restore_machines(torches);
        let bin: Vec<ItemStack> = lit.factory.get(at).unwrap().1.bin.iter().flatten().copied().collect();
        assert_eq!(bin, [ItemStack::new(torch, 5), ItemStack { wear: 600, ..ItemStack::new(torch, 2) }], "a worn stack of two kept worn");
        saved[0].bin = vec![("base:iron_pickaxe".into(), 3)];
        saved[0].bin_wear.clear();
        let mut old = crate::test_server();
        old.set_block(at, chest);
        old.restore_machines(saved);
        let bin: Vec<ItemStack> = old.factory.get(at).unwrap().1.bin.iter().flatten().copied().collect();
        assert_eq!(bin, [ItemStack::new(pick, 1); 3], "an old stack of three, three");
        // Lying in the world.
        let lying = crate::save::SavedItem { item: "base:iron_pickaxe".into(), count: 3, position: server.spawn.to_array(), age: 0.0, owner: None, lifetime: None, wear: 0 };
        let items = crate::items::Items::restore(vec![lying], |name| blocks.id(name));
        let saved = items.saved(|item| blocks.def(item).name.clone());
        assert_eq!(saved.iter().map(|i| i.count).collect::<Vec<_>>(), [1, 1, 1]);
    }

    /// A stick held in any fire catches: the last one comes back a firebrand in the hand
    /// that held it, from a campfire or a torch alike; from a bundle, one is lit and the
    /// rest stay in hand.
    #[test]
    fn a_stick_held_in_any_fire_becomes_a_firebrand() {
        for fire in ["base:campfire", "base:torch"] {
            let (mut server, _client) = holding("base:stick", 0);
            let at = put(&mut server, fire);
            server.cook(1, at);
            let brand = server.content.blocks.expect("base:firebrand");
            assert_eq!(server.players[&1].inventory.slots[0], Some(ItemStack::new(brand, 1)), "lit at {fire}, in hand");
        }
        let (mut server, _client) = holding("base:stick", 0);
        let stick = server.content.blocks.expect("base:stick");
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(stick, 3));
        let at = put(&mut server, "base:campfire");
        server.cook(1, at);
        let inventory = &server.players[&1].inventory;
        assert_eq!(inventory.slots[0], Some(ItemStack::new(stick, 2)));
        assert_eq!(inventory.count(server.content.blocks.expect("base:firebrand")), 1);
        let (mut server, _client) = holding("base:stick", 0);
        let at = put(&mut server, "base:stone");
        server.cook(1, at);
        assert_eq!(server.players[&1].inventory.slots[0], Some(ItemStack::new(stick, 1)), "no fire, no firebrand");
        // What tops up a stack already carried goes there, not into the emptied hand.
        let (mut server, _client) = holding("base:cobblestone", 0);
        let stone = server.content.blocks.expect("base:stone");
        server.players.get_mut(&1).unwrap().inventory.slots[5] = Some(ItemStack::new(stone, 3));
        let at = put(&mut server, "base:pit_kiln");
        server.cook(1, at);
        let inventory = &server.players[&1].inventory;
        assert_eq!((inventory.slots[0], inventory.slots[5]), (None, Some(ItemStack::new(stone, 4))));
    }

    /// A firebrand burns a second at a time wherever it is carried, held or not, and burnt
    /// out leaves a charred stick; the once-a-second pass burns it by the server's clock.
    #[test]
    fn a_firebrand_burns_out_to_a_charred_stick() {
        let (mut server, _client) = holding("base:firebrand", 0);
        let (brand, charred) = (server.content.blocks.expect("base:firebrand"), server.content.blocks.expect("base:charred_stick"));
        let lasts = server.content.tools.burns(brand);
        server.players.get_mut(&1).unwrap().inventory.slots[20] = Some(ItemStack::new(brand, 1));
        server.burn_carried(lasts - 1);
        for slot in [0, 20] {
            assert_eq!(server.players[&1].inventory.slots[slot], Some(ItemStack { wear: lasts - 1, ..ItemStack::new(brand, 1) }), "slot {slot}");
        }
        let mut pass = super::Burning::default();
        for _ in 0..(voxmar_protocol::TICKS_PER_SECOND as u64 / voxmar_engine::schedule::Tier::Low.period()) {
            crate::process::Process::run(&mut pass, &mut server);
        }
        for slot in [0, 20] {
            assert_eq!(server.players[&1].inventory.slots[slot], Some(ItemStack::new(charred, 1)), "slot {slot} burnt out");
        }
        server.burn_carried(lasts);
        assert_eq!(server.players[&1].inventory.slots[0], Some(ItemStack::new(charred, 1)), "a charred stick burns no more");
    }

    /// Monsters keep off where a player holds a light (a firebrand), as off a lit torch:
    /// the spot by them counts as lit, and only while it is in hand and they live (one dead
    /// burns no light down, so lights nothing).
    #[test]
    fn a_held_firebrand_lights_the_spot_for_spawning() {
        let (mut server, _client) = holding("base:firebrand", 0);
        let cell = server.players[&1].position.floor().as_ivec3() + IVec3::new(3, 0, 0);
        let lit = server.lit_near(cell);
        server.players.get_mut(&1).unwrap().health.current = 0;
        assert!(!server.lit_near(cell), "held by the dead, it does not");
        server.players.get_mut(&1).unwrap().health.current = 10;
        server.players.get_mut(&1).unwrap().selected = 1;
        assert!(lit, "held, it lights");
        assert!(!server.lit_near(cell), "carried, it does not");
    }
}
