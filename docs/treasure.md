# Treasure: locked chests and lockpicks

Part of P61 (repairs, locks, relics and gliders). The code is `crates/engine/src/treasure.rs`
(the rules and the tables) and `crates/server/src/treasure.rs` (picking a lock). The data is
`assets/content/base/treasure.ron` (picks, chests and tables), `blocks_treasure.ron` (the
chest blocks and the lockpicks) and `recipes_treasure.ron` (making lockpicks).

## The rules it keeps

- **Locks follow the ages.** A chest is locked by the age it belongs to (1 Nomad, 2 Farm,
  3 Bronze, 4 Iron, 5 Steam, ...). A lockpick of the age before, or any later, opens it and
  is used up. One two ages short opens it only on a lucky try (`lucky`, 0.05), and breaks
  whether or not it does. Anything cruder does not try, and is kept.
- **Nothing breaks a locked chest.** Chest blocks are tagged `locked` and have a hardness
  below 0; the registry refuses one that is not both, and a `locked` block with no chest.
- **Opened, it is a plain chest.** The block becomes `opens_to` (a chest), holding what its
  table rolled. The roll is seeded by the world's seed and where the chest stands, so the
  same chest in the same world held the same. The registry refuses a table that could give
  more stacks than its chest holds.
- **Never a whole set, nothing too rare.** A table's `loot` entries come on their own
  chances (the format of `loot.rs`); each `one_of` group gives, on its chance, one of its
  items, which is where gear goes, so a chest holds at most one piece. Nothing may come in
  fewer than one chest in 400: the registry refuses a table that would. A rarer mark
  comes less often than a plainer one: the registry refuses a table where any rarer mark
  is not below every plainer mark (including unmarked chances of 0.1 or less). Chest
  tables refuse `when` and `tools`, since a chest roll has neither a kill nor a held tool.
- **Gold by worth.** A chest always holds gold: as much as a Common thing of its `level` is
  worth (value.rs's price curve), times a share between `centre - spread` and
  `centre + spread` (`gold` in treasure.ron: 1.5 and 0.5, so one to two times that worth),
  in full stacks. The registry refuses a centre not above the spread, or a spread below 0,
  and counts the most gold stacks in what a chest must have room for.

Right click a locked chest (`base:pick_lock`) with the lockpick in the selected slot. With
anything else in hand, the player is told it is locked and what opens it.

## What there is

| Age | Chest (level) | Lockpick (Crafting) | Holds |
| --- | --- | --- | --- |
| Nomad | Nomad Treasure Chest (5) | Bone (5) | 3-6 gold; flint, twine, food, bone picks; a stone tool or weapon 3 chests in 10 |
| Farm | Farm Treasure Chest (15) | Copper (18) | 5-10 gold; copper, seed mix, vials, bone picks; a copper tool, weapon or rod 3 in 10 |
| Bronze | Bronze Treasure Chest (25) | Bronze (24) | 9-18 gold; bronze, charcoal, copper picks; a bronze weapon, pickaxe or fine rod 3 in 10 |
| Iron | Iron Treasure Chest (37) | Iron (30) | 18-36 gold; iron, coke, bronze picks; an iron tool or weapon 3 in 10; a grave ward 1 in 100 |
| Steam | Steam Treasure Chest (52) | (iron) | 45-90 gold; plates, coke, iron picks; iron gear 3 in 10; a grave ward 1 in 50; a diamond 3 in 100 |

A pick is two to a craft: a pin of its material on a twine grip. Chests hold a pick of the age before their own (Nomad holds bone). A found pick can
reach the next chest only on the rare lucky try; the next reliable pick costs its Crafting
level, so found picks never form a ladder through the ages.

## Placing chests

Worldgen places the blocks (ruins, villages, caves): a chest block is all it needs. A mod
adds an age's chest with a block tagged `locked` (hardness below 0), a `chests` entry and a
table, and its pick with a `picks` entry.

## Tests

- `treasure::tests::the_games_chests_hold_nothing_rarer_than_one_in_400_and_never_two_pieces_of_gear`
  opens ten thousand of each of the game's chests.
- `treasure::tests::a_chest_gives_at_most_one_of_a_group_and_bad_tables_are_refused`
  checks gold's range and stacks, and refuses bad gold and out-of-order rarity marks.
- `a_treasure_chest_opens_to_a_pick_of_its_age_or_the_one_before_and_holds_its_tables_finds`
  (gameplay) tries each kind of pick on a chest in a running server, counts its gold
  against the level's worth, and mines at a locked chest that stays.

Later packages replace `lucky` and `gold`, with a load note naming both packages
(Design draft 91). The chest and pick definitions stay keyed by block/item, the tables by name.
