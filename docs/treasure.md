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
  fewer than one chest in 400: the registry refuses a table that would.

Right click a locked chest (`base:pick_lock`) with the lockpick in the selected slot. With
anything else in hand, the player is told it is locked and what opens it.

## What there is

| Age | Chest | Lockpick (Crafting) | Holds |
| --- | --- | --- | --- |
| Nomad | Nomad treasure chest | Bone (5) | 2-6 gold; flint, twine, food, bone picks; a stone tool or weapon 3 chests in 10 |
| Farm | Farm treasure chest | Copper (18) | 4-10 gold; copper, seed mix, vials, copper picks; a copper tool, weapon or rod 3 in 10 |
| Bronze | Bronze treasure chest | Bronze (18) | 6-16 gold; bronze, charcoal, bronze picks; a bronze weapon, pickaxe or fine rod 3 in 10 |
| Iron | Iron treasure chest | Iron (30) | 10-24 gold; iron, coke, iron picks; an iron tool or weapon 3 in 10; a grave ward 1 in 100 |
| Steam | Steam treasure chest | (iron) | 16-40 gold; plates, coke, iron picks; iron gear 3 in 10; a grave ward 1 in 50; a diamond 1 in 100 |

A pick is two to a craft: a pin of its material on a twine grip. Chests hold the next pick
sometimes, so a found chest can lead to the next.

## Placing chests

Worldgen places the blocks (ruins, villages, caves): a chest block is all it needs. A mod
adds an age's chest with a block tagged `locked` (hardness below 0), a `chests` entry and a
table, and its pick with a `picks` entry.

## Tests

- `treasure::tests::the_games_chests_hold_nothing_rarer_than_one_in_400_and_never_two_pieces_of_gear`
  opens ten thousand of each of the game's chests.
- `a_treasure_chest_opens_to_a_pick_of_its_age_or_the_one_before_and_holds_its_tables_finds`
  (gameplay) tries each kind of pick on a chest in a running server.
