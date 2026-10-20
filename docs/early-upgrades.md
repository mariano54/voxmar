# Gathering upgrades and multi-part quests (Nomad and Farm)

Oct 13, 2026. Mariano's asks: the first two ages should have common-resource collection with
upgrades quick enough that gathering feels faster or richer; upgrades should also make the
home cosier and more useful; the quest system takes structure from GTNH (never its text or
assets); a quest can have several parts, with collecting and crafting built in; and each
piece of content should ask how a player might use it with other content, and answer in
data. This note is the gathering and quest side; the home ladder itself is the Cozy home
thread's (docs/home.md).

## The pattern: a quest taught you something, and it stays

Tools are a ladder in data (`blocks_tools.ron`, `tools.ron`): wooden (speed 2), stone (4,
yield 0.05), copper (7, yield 0.1), iron (14, yield 0.15), in five kinds (speeds from the
Oct 14 tier audit, docs/tools-and-crafting.md). That ladder was
quick already; what was missing was a reward for climbing it that the player *feels*. Now an
effect can wait on a finished quest (`Done`, docs/status-effects.md), so finishing the
gathering quests is itself the upgrade:

| Quest | Perk (base/effects.ron) | What changes |
| --- | --- | --- |
| Stone in Hand (24 s into a fresh game) | Steady hands | every tool bites a tenth faster |
| A Full Stone Kit | Practised hands | a fifth faster, an extra piece 3 times in 100 |
| Copper Kit (Farm) | Copper hands | nearly a third faster, an extra piece 6 times in 100 |
| A Pile of Stone | Stonewise (mountain biomes) | an extra piece 5 times in 100 more |
| A Fish Supper | Angler's knack | casts land a tenth more often |
| Patient Angler (Farm) | Deft angler | casts land a fifth more often (replaces Angler's knack) |
| An Ore Run (Farm) | Prospector | a twentieth faster, an extra piece 4 times in 100 more |
| Food Enough (Farm), at home | Settled and fed | heal a tenth faster within reach of home |

Each step of the ladder says so in chat once and names the next quest to climb
(`Next: carry a full stone kit...`). They stack with the tool, the skill, food and the place
(Rich rock, Waterside), so a copper pickaxe in the mountains with a hearty meal is a real
step up from a stone one on a plain. No new item, no art, all numbers in data, and a mod
adds its own perk by adding an effect that waits on a quest.

### Other content it meets (the interaction pattern)

- **Where**: perks pair with biomes with `All([Done(..), InBiome(..)])` (Stonewise). The
  same shape works for weather, a home (`AtHome`, `HomeHas`) or the time of day.
- **Food and rest**: the modifiers multiply with Hearty, Focused, Well rested and Deep rest,
  so cooking and a good bed make a gathering session better.
- **Quests that count steps**: reaching a ladder step is `Built("knack_NAME", 1)`, so a
  later quest can ask for a step; the home ladder uses the same door
  (`Built("home_snug", 1)`).
- **Mods**: `Done` names any package's quest; a mod's quest can unlock a core or base perk.

## Multi-part quests

A task is a list of goals that must all hold, with alternatives as separate tasks, so
collecting and crafting already live in one quest. The chain GTNH-style is a tree of parts
leading to a step:

| Quest | Age | Parts (all of them) |
| --- | --- | --- |
| Stone in Hand | Nomad | craft a stone pickaxe and axe |
| Dig In | Nomad | craft a stone shovel, then have 4 clay |
| A Full Stone Kit | Nomad | carry a stone pickaxe, axe, shovel, hoe and knife |
| Stockpile | Nomad | carry 32 logs and 64 cobblestone |
| Copper Kit | Farm | craft a copper axe, shovel, hoe and knife |
| An Ore Run | Farm | mine 24 copper, 9 tin and 12 coal |
| A Stove and a Quern | Farm | craft a cottage stove and a workshop piece |
| Books and Banners | Farm | craft a bookshelf and a banner |
| Make Camp, Snug for Winter, A Homely Home, A Cosy Home, Heart of the Home | Nomad, Farm | reach each home step (any higher step counts too) |

The home steps come after the craft quests that make the pieces (seats, table, shelves,
rugs, crockery, candles, stove, bookshelf, banner), so the book shows a tree: parts, then
the step. Each pays Construction experience and plain supplies (torches, candles, bread,
coal); nothing breaks the item economy (worth stays fixed per item).

## What GTNH teaches us (structure only)

- **Chapters with a prerequisite tree and optional side branches**: ours already (age
  chapters, `after`, `branch`).
- **Multi-part quests**: see above. Nothing to build.
- **Unlocks from quests**: GTNH's quests gate what you may do next; ours unlock perks and
  open the next quests.
- **Detect versus consume**: ours only detects (`Have` counts what you carry). A consuming
  hand-in (`Give(item, n)`: taken from the pack when the quest finishes) is not built.
- **Choice rewards** (pick one of several): not built; rewards are all-or-nothing.
- **A visible next step**: the quest book pins (seeded for a new player) and the Next line.

## Not built, in the order worth doing

1. More perks, as data only: a `Done` perk for Farm quests (a hoe's seed yield, a flock's
   wool), once the first ladder has been played and found too slow or too fast.
2. `Give(item, n)`, a consuming goal, so a quest can ask for a stock of materials and take
   them (the stockpile as a tribute to the settlement). Needs a reservation so a partly met
   task never takes items, and a server message for what was taken.
3. A choice reward (`Reward::Choice`), with a pick in the quest book (protocol change).
4. An effect-icon for each perk (they use the plain buff icon).
