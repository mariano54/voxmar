# Gathering upgrades and multi-part quests (Nomad and Farm)

Oct 13, 2026. Mariano's asks: the first two ages should have common-resource collection with
upgrades quick enough that gathering feels faster or richer; upgrades should also make the
home cosier and more useful; the quest system takes structure from GTNH (never its text or
assets); and a quest can have several parts, with collecting and crafting built in. The home
ladder is another thread's (Cozy home upgrades); this note is the gathering and quest side.

## What already speeds gathering up

Tools are a ladder in data (`blocks_tools.ron`, `tools.ron`): wooden (speed 2), stone (4,
yield 0.05), copper (6, yield 0.1), iron (8, yield 0.15), in five kinds. Stone is one
cobblestone and sticks at Engineering 1, so a new player is four times a bare hand within
the first hour; copper (Engineering 5, Mining 5 for the ore) is the Farm age's step. Skills
raise finds on top. So the ladder is quick already; what was missing was telling the player
to climb it, in steps with a reward.

## The quests that make the climb visible

| Quest | Age | Parts (all of them) | Why it is there |
| --- | --- | --- | --- |
| Stone in Hand | Nomad | craft a stone pickaxe and axe | first 4x step |
| Dig In | Nomad | craft a stone shovel, then have 4 clay | clay is the Hearth's missing material |
| A Full Stone Kit | Nomad | carry a stone pickaxe, axe, shovel, hoe and knife | the whole 4x kit |
| Stockpile | Nomad | carry 32 logs and 64 cobblestone | common-resource stock to build with |
| Copper Kit | Farm | craft a copper axe, shovel, hoe and knife | the 6x step with more yield |

Each pays Engineering or Construction experience and plain supplies (torches, bread, coal),
nothing that breaks the item economy (worth stays fixed per item).

## What GTNH teaches us (structure only)

- **Chapters with a prerequisite tree and optional side branches**: ours already (age
  chapters, `after`, `branch`).
- **Multi-part quests**: a task is a list of goals that must all hold, with alternatives as
  separate tasks, so collecting and crafting already live in one quest (Dig In, Food Enough,
  Stockpile). Nothing to build.
- **Detect versus consume**: ours only detects (`Have` counts what you carry). A consuming
  hand-in (`Give(item, n)`: taken from the pack when the quest finishes) is not built.
- **Choice rewards** (pick one of several): not built; rewards are all-or-nothing.
- **A visible next step**: the quest book pins (now seeded for a new player).

## Not built, in the order worth doing

1. `Give(item, n)`, a consuming goal, so a quest can ask for a stock of materials and take
   them (the stockpile as a tribute to the settlement). Needs a reservation so a partly met
   task never takes items, and a server message for what was taken.
2. A choice reward (`Reward::Choice`), with a pick in the quest book.
3. Yield and speed upgrades beyond tool tiers (whetstones, bellows, a hoe's seed yield),
   once the tiers have been played and found too slow or too fast.
