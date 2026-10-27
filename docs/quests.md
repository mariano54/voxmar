# Quests

Oct 7, 2026. The quest engine (the progression plan's P20): quests as content, checked
against what a player has done, so they complete retroactively. The quest book screen
(P21) is the interface's; this is its data.

## Rules

- Nothing is locked behind a quest. Recipes stay open; quests point the way and reward
  the walk with experience, items and (later) knowledge.
- A quest completes the moment one of its tasks holds, however long ago the work was
  done: the goals read the player's state, not a log of events since the quest appeared.
- A task is a list of goals that must all hold; a quest offers one task per route (craft
  it, find it, trade for it).
- `after` names the quests that come first; `"a|b"` takes either. Finishing one quest can
  finish others that were already done, in the same moment.

## Content

`base/quests.ron` (engine `quest.rs`): name, age, branch, title, why (the reason and the
reward, shown first), tasks, after, rewards. A book of its own can go in a
`quests_<topic>.ron` beside it (`quests_nomad.ron`, `quests_farm.ron`), like `blocks_` and
`recipes_`: a package's files are read in name (byte) order, so `quests.ron` comes first and
the topics follow in byte order (uppercase before lowercase), and the quests are held in that
order. A quest may come `after` one in any file. A quest name written twice, in one file or
two, fails the load naming both files, across packages too: a mod cannot replace a quest by
writing its name. Quests that come after each other (one after itself, or a loop of them
with no choice out) would never open, so the load refuses them, naming the loop.
A file no part of the game reads (a misspelt `quest.ron`) still loads, and
the load notes it with the likeliest name meant (`zz_mod/quest.ron is read by nothing (meant
quests.ron?)`); the names other crates read are `content::READ_ELSEWHERE`.
Goals:

| Goal | Reads |
| --- | --- |
| `Craft(item, n)`, `Cook(item, n)` | the player's tallies of things made and cooked |
| `CookAt(item, station, n)` | successful cooking tallies for a station tag; First Night asks for edible food at a campfire, plus crafting the campfire |
| `Built(structure, n)`, `Lit(structure, n)` | standing construction and successful fuelled lighting, separately; factory construction never supplies lighting credit. `Built` also takes the announced steps of an effect ladder, written short and tallied in full: `Built("home_snug", 1)`, `Built("knack_practised_hands", 1)` (the effect's `ladder` and name; docs/home.md, docs/status-effects.md) |
| `Tame(creature, n)` | the tally of animals tamed |
| `Catch(fish, n)`, `Catch("Kinds", n)` | the journal's Fish page |
| `Kill(creature, n)` | the journal's Animals and Monsters pages |
| `Pick(plant, n)`, `Mine(ore, n)` | the journal's Plants and Ores pages |
| `Visit(n)`, `Discover(page, n)` | how many entries the journal has (Animals and Monsters entries are only found from close up, see [journal.md](journal.md)) |
| `Kinds(page, which, n)` | how many different entries of a page were done at least once (killed, caught, picked, mined; only seen does not count), among `any`, a `tag:TAG` or one name: Wild Harvest is `Kinds(Plants, "any", 5)` |
| `Level(skill, n)` | the skill's level |
| `Have(item, n)` | what the player carries, pouches too |
| `Home` | a lit Homestead Hearth |
| `LookUp(item)` | the recipe viewer was opened on the item (the client says so once, `ClientMessage::LookedUp`; tally `lookedup:<item>`) |
| `Thriving(n, percent)` | the player keeps n animals right now at a condition of at least that percent (read live from the kept animals, not tallied; a newly tamed one starts at 60) |
| `Met(kind, n)`, `Met("Kinds", n)` | dealings with wandering parties (docs/wandering.md), tallied `met:<kind>`: a line said that gave or took something, a show watched, a share of a hunt, a captive's or a guided one's thanks, a lure followed to its end; a greeting alone is none. `any` counts every kind; `Kinds` counts the kinds dealt with at least once |
| `Dealt(kind, line, n)` | dealings with one line of a wandering kind (`dealt:<kind>/<line>`), counted as `Met` counts but that line only, for a lesson one line alone teaches (the herb gatherer's roots, the hedge witch's mote); load-checked against the kind's lines in wandering.ron |
| `Prayed(prayer, n)` | prayers turned on (`prayed:<prayer>`); one already on is not turned on again |
| `Offered(item, n)` | what was offered at an altar (`offered:<item>`), an item, `any` or a `tag:TAG` |
| `Ate(food, n)` | food eaten (`ate:<item>`), an item, `any` or a `tag:TAG` |
| `Felt(effect)` | whether the player has ever been under an effect, whatever gave it (`felt:<effect>`, set once by the survival tick) |

Items and creatures may be `any` or `tag:TAG`. Rewards are `Xp(skill, xp)` and
`Item(item, count)`. The experience a quest file writes is scaled as the quests load by
`pays` in `core/quests.ron` (Oct 21, Mariano: bigger quest rewards): `xp: 2.0` doubles every
quest's experience, and `capstone_xp: 1.5` pays a capstone half as much again, 3x in all
(`xp` times `capstone_xp`). What the server pays, the book and tracker show, and the walks
count is the scaled number; a later package changes either field. Titles and reasons are translated as `quest.<package>.<name>` and
`quest.<package>.<name>.why`.

## On the server

Each second the server checks every player's open quests, finishes those whose task
holds, tells the player with a chat line, and sends `ServerMessage::Quests { states }`
when any state changed: each quest's name, whether it is done, whether it is claimed, and
each goal's progress as (have, need). The client keeps them in `LocalPlayer::quests`.
Quests done, quests claimed and the tallies are saved with the player.

## Claiming

Finishing a quest pays nothing by itself. A done quest with rewards waits in the quest
book: its shape on the web is gold with a "!" in its corner, its chapter shows a gold "!", the chat says so, and its
page has a Claim button (`ClientMessage::ClaimQuest`) beside the rewards, which list the
experience and the items. The server then gives the items and the experience once. A quest
that pays nothing is claimed as it finishes. If the pack cannot hold every item, nothing
is given, the player is told, and the quest stays claimable; the reward never drops on the
ground. A quest that is not done, was claimed, or does not exist is ignored.

Unclaimed rewards persist: the save lists the claimed quests (`quests_claimed`). A save
from before claiming has no list, and counts everything it finished as claimed, since all of
it was paid then.

The sounds: a jingle as a quest is finished and coins into the purse as it is claimed,
played by the client as the server's states change (not for the states a session starts
with). A path's last quest (its capstone, `capstone: true`) rings its own, warmer tune
instead of the quest's. A skill's new level rings too, and every tenth
level a grander tune. They are recordings the base package's `sounds.ron` lists
(`quest_done`, `quest_claim`, `level_up`, `level_milestone`, `capstone_done`), our own, composed by `tools/generate_jingles.py` (short brass fanfares
played from the FluidR3 sound font, and synthesized coins); a later package that
lists any for an event replaces them. They follow the sound volume, not the choice of click
sounds.

A quest finished and a level reached are each a moment (`hud/celebrate.rs`): a banner pops
in at the top of the screen (the quest and where to claim it, or the skill, its level and
what that opens), motes burst around the player, and the jingle plays as it shows. A
milestone level (every tenth) is grander. All of it is `core/celebrations.ron`. Moments
take turns: a quest finished by reaching a level shows first, and the level cuts in once
the quest has held for its `hold`.

The Nomad and Farm chapters (ages 1 and 2) have the quests the game can already do:
Nomad runs from the first campfire to the lit Homestead Hearth, Farm from the first harvest
and the pit kiln to the lit Crucible Kiln. Each has at least thirty quests
(`crates/engine/tests/early_quests.rs` keeps that floor and checks every name they use). The
loop through the first days, and what a new player is and is not told, is in
[first-days.md](first-days.md). The rest of quest book v2 joins as its content lands.

The Crucible Kiln needs a standing pattern and a complete charge held in the hand:
32 charcoal or 24 coal. Right-clicking its core lights it and consumes that charge.
Its burn time uses the fuel times in `machines.ron`, measured by server ticks; the
remaining fire is saved in `world.ron`. Cooking needs both the standing pattern and a
live fire. Removing a required piece quenches it; rebuilding requires a new charge.
Completed quests in older saves stay completed. Older `cooked` and `built` tallies do
not establish which station cooked food or whether fuel was paid, so they cannot
complete these stricter goals for an unfinished quest.

A package's `tracker` may name `start_pins`: quests pinned for a player whose book is new
(nothing done, nothing pinned), once per install (remembered in the settings). A later package replaces the list.

## Multi-part quests

A task is a list of goals that must all hold, so one quest can ask for a gather, a craft
and a cook together. Quests that were a single count are two-part where the second part is
what the player does next anyway (Oct 13, Mariano: "a bit too simple"): Small Game (kill,
then cook one), Hide and Seek (hides and bones), Rod and Line (craft, then land a fish),
A Fish Supper (catch three, cook three), Meat on the Fire (six cooked, Cooking 2), Sharpening
Up (Attack 4 and five kills), First Harvest and Carrot Patch (harvest, keep the crop), A
Field of Gold (24 harvests, Farming 4), Herb Garden (planter and six plants), Seed Mix (the
mix; its wheat ask went with A Field of Gold before it, Oct 21), Fresh Eggs (fry the first egg, then keep five), A Woolly Flock (tame,
and shears made), The Milk Pail (the pail, then the milk), A Silvery Ore (tin; its copper
ask, already met by Red Metal, went Oct 21), Many Kinds (three kinds, six cooked). The main line of each age (the pacing
test's milestones) stays as it was. Rewards are unchanged.

## Quests that meet the rest of the game

Oct 13: twenty-two quests tie the first two ages to what came after the quest book was written,
so that what the game already does is something the book points to:

- **Wanderers** (`Met`). Nomad: Folk of the Road (deal with anyone), A Kind Turn (feed the lost
  child or the hurt hunter), Something for Something (a pedlar, tinker, berry pickers,
  woodcutters or charcoal burners). Farm: Farmers on the Road (the harvest family, goose
  girl, cowherd, water carriers or shepherd), Safe Home (refugees, the prison convoy or the
  hurt traveller) and Many Travellers (five kinds). Each route names a kind, so the hint is
  true; parties are rare, so the routes are alternatives rather than a list.
- **Home rooms** (`Built` on a room's step). Nomad: A Bench to Work At, Pantry Shelves,
  Company at Camp. Farm: A Proper Workshop, A Larder, A Kitchen Garden, A Full House. Each
  room's first step is Nomad's, its second Farm's, as `effects.ron` lays them out.
- **The altar** (`Offered`, `Prayed`). Altar Rites (three bones offered and Bark Skin turned
  on) and A Prayer in the Fight (Ember Arm and Bark Skin on, ten creatures defeated).
- **Cooking**: A Varied Table (meat, fish and bread together).
- **Batch 9 wanderers** (`Met` on one kind each). Nomad: the market folk (sell wheat to
  them on the road, or walk with them to town: the arrival counts too, but only for those
  who kept them company; now the first part of The Market Road) and Caught in the Rain (warm the drenched travellers with a
  torch, or share food). Farm: Neighbourly (the neighbours call once the hearth is lit),
  The Rain Dance (watch the rainmakers or pay them), Filled Again (the shrine bearer) and A
  Share of the Hunt (strike first at a hunting party's, trapper's or wolf hunters' quarry).
- **Wanderer quests in two parts** (the hooks in docs/wandering.md, Oct 13). Each pairs a
  wandering kind with what the player does next anyway, so meeting them is a side step,
  never a wait. Nomad: Coal by Road (the coal carters or the benighted miners, and sixteen
  torches; pays four coal). Farm: Fired Whole (the clay diggers fire your clay; pays six
  gold coins), The Last Sheaf (the harvest home procession; pays eight wheat seed) and Mice in
  the Stores (follow the piper to the end of his tune; pays four seed mix). Their clay, wheat
  and larder asks went on Oct 21: A Kiln Load, A Field of Gold and A Larder before them had
  always done them already.
- **Wanderers that come back** (the Nomad thread's hooks, Oct 13): quests that ask for a
  kind and the kind that follows it, all Farm side branches. The Keepers' Word (the
  poacher, the gamekeepers on his trail and, days later, his gang), Catch Up the Shrine (the
  stragglers asking after the shrine bearer Filled Again met), From Wedding to Christening, Those
  Taken In (refugees, then their kin), First Metal (the bloomery smelters before the
  Crucible Kiln, and four copper ore) and A Hen of Your Own (the henwife's hen, kept near
  home). Three more from batch 16: Master and Journeyman (Farm: the apprentice
  who asks to learn your best skill at 15, then the journeyman), The Market Road (Nomad,
  after Something for Something: the market folk, then beat the highwaymen's ambush; the win is the
  dealing their tally counts) and Found and Thanked (Nomad, after A Kind Turn: the lost
  child brought to town, then the parents who come to your home).

- **Bees** (Oct 14, docs/bees.md). Farm: Humming in the Meadow (Farming 12, a skep made and a
  swarm taken into it; the hint names every biome wild bees nest in, held by a test), A Bee
  Garden (the home step and three honey; it names the Kitchen Garden and Herb Garden for the
  wait while the skep fills) and The Beekeeper Calls (the beekeeper met and three candles, or
  two beeswax kept and six candles; pays a spare skep).

- **What landed after** (Oct 14). Seven quests teach a rule no other quest says, in its
  text, and ask for the sequence that shows it. Nomad: Stones Back (a
  sling stone that hits is left lying one time in four; five creatures and Ranged 4; now
  parts of A Stone's Throw) and
  Light Sleepers (after Under Lock and Key: four Nomad chests in ten wake cave bats; two
  chests in all and two bats). Farm: Every Arrow Counts (after Bow and Arrow: half the arrows
  that hit are left lying; sixteen arrows in all and Ranged 8), What Sleeps Below (with
  Deeper Down folded in: a Farm chest may wake cave crawlers; two chests and a crawler),
  Heart of the
  Wood (after The Rootmother: Heartwood, ten minutes that another heartroot only starts
  again; eat one and keep two), Smoke That Clings (after Humming in the Meadow: a torch
  held or a fire within two leaves you Smoky; Smoky and four honey) and Sweet Relief (after
  A Bee Garden: honey heals 2 and stacks, the same food in a row heals less; eat three and
  keep three). The reach test counts a boss's loot, since its lairs place it.

- **Home stations** (Oct 14, docs/stations.md). Nomad: Hung to Dry (a drying rack made and four
  jerky kept) and Trail Rations (a second rack, twelve jerky). Farm: A Compost Heap (a heap
  made, then six crops grown with compost and one with a bone, Rich Soil's old parts; the
  goal `Fertilised(item, n)` counts the crops grown).

## Quick upgrades

Oct 13 (docs/first-days.md, "Quick upgrades on common resources"): Timber asks for four logs
with a flint axe, the planks for the table and sticks, and An Axe to It (after Crafting
Table) asks for a spare flint axe and eight logs chopped with it (Oct 15: there are no
wooden tools). First Night tells the player the stone axe hits three
times as hard as a fist.

## Structure (Oct 13)

A pass on the Nomad and Farm chapters against the way a long modpack's quest book is laid
out: each chapter a spine of main quests to one goal, side branches hanging off it, nothing
on the spine gated by a side quest, and a quest that is several steps shown as several.

| Check | Before | After |
|---|---|---|
| A clear goal per chapter | Nomad's goal Homestead Hearth ("the Farm age opens"; since Oct 15 its rite, The Farm Age, follows it and opens the age); Farm's goal the lit Crucible Kiln ("starts the Bronze age"), with First Bronze after it as the bridge to the next age | Unchanged: both goals are the last spine quest their text names as the age's end |
| Side branches read as optional | No spine quest comes after a side quest (the_potter, brick_by_brick and crucible_kiln come only after spine quests or the hearth); the wanderer quests say "may" | Unchanged |
| Nothing unreachable or a dead end | Every `after` names a quest that exists (load-checked); no Nomad quest waits on a Farm one; no Craft or Level goal asks more than Engineering 10 (The Crucible). A Place to Pray stood in the Farm tab with no `after`, so it could finish before the Farm age opened. Stone in Hand had no `after`, so the spine read as three loose starts | A Place to Pray comes after Homestead Hearth like the rest of its tab; Stone in Hand after Crafting Table, so the Nomad spine reads Crafting Table, Stone in Hand, then First Night's line (First Night stays pinned at the start, with no `after`) |
| Several steps shown as several | Homestead Hearth was one goal (`Home`) for a hearthstone, two chimneys and the lighting | Three parts: the hearthstone, two chimneys, and the lit hearth (`Home`). The book ticks each as it is made |

Left as they are: Timber stays off the spine (branch `start`, with An Axe to It), since the
main-line walk test models only the spine's goals; Crafting Table stays a root so a player
who already has a table finds it done.

## Farm paths in layers (Oct 14)

The Farm tab had 25 quests hanging straight off Homestead Hearth, so its first column was a
wall of parallel starts. Now each of ten paths starts once off the hearth and runs in
layers, with forks inside a path and few links between paths: the main line (A Pit Kiln to
the Crucible Kiln), pottery (Clay Banks), farming (First Harvest), cooking (Camp Cook),
husbandry (Hens in a Row), hunting and fighting (Bigger Game), ranged (Bow and Arrow),
fishing (Patient Angler), the road (Far Horizons), and the home (A Seat by the Fire, now part of A Table
for Two).
Prayer opens off A Pit Kiln as before. Encounter quests sit inside their path: the
hunters' and the poachers' after Bigger Game, the road's travellers after the Field Guide,
the farming folk after A Field of Gold, the neighbours after a snug home. The book's
columns go from 25, 23, 18, 12, 6, 5, 4, 2 quests to 10, 15, 21, 20, 15, 7, 5, 2: the tallest
column is four shorter. Only `after` changed. Levels asked only come later, and
early_quests still holds that a quest names only quests open beside it.

### Each path a chain (Oct 14)

Ten starts still fanned out a few steps in: one column held 21 quests. Now each path runs
as a chain, with a fork only where a quest's text sends the player two ways (The Potter
opens A Kiln Load and Earthenware, which A Kiln Load's text sends the player to between
loads, and A Kiln Load leads to Fired Whole; Herb Garden names Carrot Patch and A Kitchen Garden;
Fresh Eggs names A Woolly Flock). Examples: the bees run Humming in the Meadow, A Bee
Garden, then Smoke That Clings beside Sweet Relief and The Beekeeper Calls; the home runs A Table for
Two, Shelves and Cupboards, A Proper Bed, Candlelight, Snug for Winter, then the door and
the workshop; the road runs Field Guide, Many Travellers, Safe Home, The Rain Dance, the
wedding; ores run Copper Kit, Tin Find, Rock Collector, Ore Run. Copper Kit follows Red
Metal, the copper the main line already asks for, not First Metal: the bloomery smelters
are a wandering party, and no chain waits on meeting one.

| Measure | Main before | Ten starts | Chains |
|---|---|---|---|
| Farm quests open at once along the kiln bot's run (max, median) | 33, 31 | 17, 15 | 12, 11 |
| Open at once, clearing the whole age one quest at a time, oldest first (max, median) | 34, 19 | 25, 15 | 15, 12 |
| Columns, and the tallest | 8, 25 | 8, 21 | 11, 13 |

Along the bot's run the open quests are one head per path: the next main quest, plus Camp
Cook, Far Horizons, First Harvest, Patient Angler, A Table for Two, Earthenware, Bigger
Game, First Metal, Fired Whole and A Place to Pray.

A path waits on the stations its goals need (Oct 21): A Stove and a Quern comes after Fire
the Clay as well as Snug for Winter, since every workshop piece now takes kiln output (the
spindle and loom fired clay, the quern stone, which is cobblestone fired in a pit kiln), and
Hung to Dry comes after Crafting Table, as its rack is a 3 by 3 recipe. A content test
(`a_quest_asks_only_for_what_its_stations_are_taught_for`) holds every quest on every path to
this: one of its routes (a quest finishes on any one of its tasks) has the player make only
what is gathered, mined, or made at stations the quests before it have the player make,
counting for each goal only the ways the quest book shows for it (`recipe_book::counts`). It
prints the route it found for each quest. The main-line walk could not catch it: it walks
only the main line, and it puts down whatever station a recipe needs. What it found: Stone
in Hand has the player make the grindstone its stone heads are ground at, and A Full Stone
Kit comes after it; A Fish Supper comes after First Night, which teaches its campfire; and
the Nomad quests whose goals take hide (A Coin Purse, Pockets, A Stone's Throw, Hide Wraps)
come after Hung to Dry, whose drying rack turns a hunt's raw hide into hide. The Farm book
teaches its own rack, as a Farm quest comes after Farm quests only: Bigger Game has the player
make one, and the Farm quests that take hide (Full Hide Wraps, Light in the Window, Books and
Banners) come after it. Candlelight and The Beekeeper Calls need no hide: a candle is also a wick
over lard (a wild boar's drop) or over beeswax, so they come after their own chapters alone. A drop counts only from a creature
every player meets: the raiders and jailers that also drop hide come only with rare
wandering parties, and a boss's drop counts once a quest before has the player kill it.

### Nomad paths, and quests in parts (Oct 14)

The Nomad tab opened ten quests at once on a new world (Wild Harvest, A Pile of Stone,
Twine and Thong and the rest beside the first three), so a newcomer met a wall before
their first tool. Now four quests open at the start, the three pinned ones (Look It Up,
Timber, First Night) and the Crafting Table, and every other path starts from a step the
main line reaches: the crafting table, the stone tools, the first night or the bed. Each path runs as a chain:
mining runs A Pile of Stone and Dig In, and Black Flecks (a coal sample lifted with flint, or one coal ore, from the Crafting Table), Coal and Cinders (twelve coal, after it and Stone in Hand; Torchlight waits on it), First Ore, Stockpile; fighting runs
Something to Swing, Sharpening Up, Something Hostile, A Stone's Throw; the camp runs Make
Camp, By Rushlight, Company at Camp, Pantry Shelves, From the Shelves, Put It Away, with A Lit Corner and A Bench to
Work At beside it; grain runs A Handful of Grain, Daily Bread, Wild Harvest, Herb Lore.

Siblings that are one job are now one quest in parts. A task was already a list of parts
that must all hold (Homestead Hearth asks a hearthstone, two chimneys and the home); the
book lists each part with its own mark and count. Seven pairs became one:

| Quest | Parts | Was |
| --- | --- | --- |
| A Stone's Throw (Nomad) | a sling, Ranged 4 (its five kills, Sharpening Up's already, went Oct 21) | A Stone's Throw, Stones Back |
| Wild Harvest (Nomad) | five kinds of plant, five herbs | Wild Harvest, Herb Gatherer |
| The Market Road (Nomad) | the market folk, the highwaymen | To Market, The Market Road |
| A Compost Heap (Farm) | a heap, six crops grown with compost, one with a bone | A Compost Heap, Rich Soil |
| A Table for Two (Farm) | two seats, a table | A Seat by the Fire, A Table for Two |
| A Rug Underfoot (Farm) | two cloths, a rug | Wool and Dye, A Rug Underfoot |
| What Sleeps Below (Farm) | two Farm chests, a cave crawler | Deeper Down, What Sleeps Below |

Each keeps both texts and both rewards. Nomad has 60 quests, Farm 95 with A Pot on the Fire
(after The Potter and Camp Cook: a cooking pot made, two porridge kept), A Hearty Stew
(after it: Cooking 14, two stew kept) and A Well by the
Field (after A Field of Gold: make a well and set it by tilled ground; the new goal
`Watering(block, n)` counts a block tagged `waters` set down within four blocks of tilled
ground, as `crop_conditions` reads it, `a_well_set_by_tilled_ground_counts_as_watering_it`). A task with no part
is refused at load (`a_quest_with_an_empty_task_is_refused`), as is a part the engine
does not know.

| Measure | Nomad before | Nomad after | Farm before | Farm after |
|---|---|---|---|---|
| Open at once along the kiln bot's run (max, median) | 17, 14.5 | 9, 6.5 | 13, 8 | 13, 8 |
| Open at once, clearing the age one quest at a time (max, median) | 16, 11 | 13, 9 | 15, 12 | 15, 11 |
| Starts (quests after nothing in the age) | 10 | 4 | 8 | 8 |
| Columns, and the tallest | 7, 14 | 8, 13 | 11, 13 | 10, 13 |

These counts come from the quest data and twelve logs of the kiln bot; along the bot's run
a Farm count includes every quest open since the hearth was lit.

## Hooks for the new features (Oct 14)

Each new feature has a quest that teaches it, in two parts the book counts one by one:

| Quest | Age, path, after | Parts |
|---|---|---|
| A Chopping Block | Nomad, start; An Axe to It, A Bed of Your Own (Crafting 2 by then) and Timber | one set down; twelve planks split by it |
| Set a Snare | Nomad, hunter; Twine and Thong | one set down; a catch taken from a trap |
| Old Tusk | Nomad, hunter; Hide and Seek and Raise Your Guard | a flint spear made; Old Tusk killed |
| Light in the Window | Farm, home; A Homely Home | two hide windows made; two set down |
| A Trough for the Pen | Farm, husbandry; Hens in a Row | a feed trough set down; eight items put in it |
| A Full Granary | Farm, farming; A Well by the Field | 64 wheat plants harvested; 32 wheat kept |

Twine and Thong also takes ten twine carried, since a knife cuts twine from tall grass, and
its text and A Sharp Edge's say so. First Harvest says a stone hoe reaps the ripe crops around the one cut, and A Full Granary is the harvest that pays for it. Four goals came with them, each reading a tally:
`Placed(block, n)` (blocks set from the hand that still stand, by the item: `placed`,
tallied only for a block a `Placed` of a quest the player has not finished names; the world
keeps each such cell with its owner and item (`WorldMeta::quest_placed`), and the block
counts while the one in its cell still drops that item, so a sprung snare still stands and
a snare set over itself counts once; broken, built over or washed away, it comes off,
straight away if its owner is playing, else as they join, when their `placed` tallies are
counted again from the world's cells; a quest finished drops the cells no open quest still
counts),
`Stocked(item, store, n)` (that item put in a store of that block, or of any store block
with a `tag:` store, and left there: `stocked:STORE:ITEM`, in `push_store` and `put_away`,
net of what is taken back out), `PutAway(item, n)` (put away into a home's stores with
the Put away key: `put_away:ITEM`, in `put_away` only; Put It Away asks it), `CraftNear(item,
near, n)` (made by a recipe with `near`, by that tag: `crafted_near:TAG`, beside `crafted`)
and `Trapped(creature, n)` (a catch taken from a sprung trap: `trapped`; whoever empties
the snare gets the credit and the catch, as with a shared lit fire, until traps have
owners). Loading refuses a `Stocked` store that is not a store (or a tag no store has), a `CraftNear` tag no
recipe is made near, and a `Trapped` creature no trap catches. A tally goal naming one
full name reads its one key; only `any` and `tag:` look across a kind's keys. Nomad has 68 quests, Farm 99, each counting its rite.

## An age's rite (Oct 15)

The next age's stations wait for its rite. A quest with `opens: Some("farm")` is the rite
of that age (one to an age, and never its own), and a recipe with `age: "farm"` is
refused, on the server (`allowed_recipes`) and greyed in Can Make and the grid ("opens in
the Farm Age"), until its rite is done. A recipe naming an age no quest opens fails to
load. Two goals count the age's own quests, read from what is done rather than a tally:
`QuestsDone(age, n)` and `Capstones(age, n)`, the latter only quests marked
`capstone: true` (a path's last quest). Loading refuses a count that is 0, of an age no
quest is in, or more than the age holds besides the rite.

| Rite | After | Parts | Opens |
|---|---|---|---|
| The Farm Age | Homestead Hearth | 20 Nomad quests; 3 Nomad path capstones | pit kiln, crucible, herb planter, stone altar, earth clamp, cooking pot, well, cottage stove, compost heap, skep |
| The Bronze Age | Crucible Kiln | 30 Farm quests; 4 Farm path capstones | (nothing yet) |

The Farm quests that followed Homestead Hearth follow The Farm Age.

- **Old saves.** A rite also finishes when any quest of the age it opens is done
  (`QuestRegistry::behind`), so a save from before rites that had reached the Farm age keeps
  its stations. That is safe only while no quest of the age can be done before its rite, so
  loading refuses a quest of the age that any way round skips the rite (every `after` is
  needed, so one that is the rite or follows it is enough; of an `a|b`, both must).
- **Refused out loud.** A locked recipe crafted in the grid or laid from the book is refused
  with `craft.age` ("That opens in the Farm Age"), listed under `sent` in `refusals.ron`.
  Can Make shows it greyed ("Cooking Pot opens in the Farm Age") once half of it is held, after
  everything that is only short of something, so it never crowds out what can be made.
- **Capstones are marked.** A quest with `capstone: true` has a gold corner on its cell in the
  book and a line in its detail ("A path's capstone, gold at its corner"); the rites' texts
  point to the mark. The gate's `quests: capstones` checks each one drawn carries it.
- **A capstone closes its whole path.** It waits, through its `after` chain, on every quest
  of its age and branch but the age's rite and the side quests (`side: true`: chance meetings,
  conveniences such as pouches); loading refuses one that does not, naming the quests left
  out. The Farm book's side marks are placeholders until its own pass (piece 13) chains it.
- **A wait names what to do meanwhile.** `meanwhile: [...]` lists the quests a quest's text
  gives to do while it waits (A Kiln Load's firing: Earthenware, A Pot on the Chimney, A
  Place to Pray). Loading refuses one done before it, one not open while it is (each
  `after` a quest it waits on), or one its `why` does not name in brackets; a client test
  checks each language's text names the translated title. A Pot on the Chimney now opens
  with The Potter, beside Earthenware, so it is open during the first load. Brick by Brick's names A Table for Two, Charcoal's A Place to Pray, and The Potter's Fuel
  to Hand.
- `/quest <name>` (admin) marks a quest done, its rewards left to claim. The kiln bot uses
  it for The Farm Age until piece 12 has it earn the rite.

## The Farm book (Oct 20)

The Farm quests stay in `base/quests.ron`'s Farm block, and the road to the lit Crucible
Kiln is laid out like a long modpack's book: several paths that each make one part of the
kiln, meeting at the end, with a way round at each fork. Each step is something done in the world (gathered, fired, stored, set down), not
only crafted.

| Path | Quests to the kiln | Ways round |
|---|---|---|
| Main | A Pit Kiln (made and set down), Fire the Clay, The Potter, Clay in Store (thirty-two clay put by in a chest, a barrel or a crate), First Bricks, Brick by Brick, The Kiln Ring (twelve bricks set down), The Crucible (made and set down), Crucible Kiln; then Red Metal, smelted in the lit kiln, and First Bronze | the store a chest, a barrel or a crate |
| Fuel (new) | Fuel to Hand, Charcoal, Plans for the Kiln (the crucible and kiln bricks looked up), A Full Charge (capstone); side quests Opened Too Soon (four part-burnt wood from a clamp opened early, after A Second Clamp) and, after A Table for Two on the Home path, A Woodpile (a woodpile set down by the hearth, half a Hearth corner) | logs carried or coal mined; charcoal burnt or coal mined; the charge in charcoal or coal |
| Pottery | A Kiln Load, then Fired Whole (capstone) after Clay Banks, Earthenware and An Earth Clamp; side quests A Lamp of Clay (two oil lamps made and set down) and A Pot on the Chimney (chimney pots made, one set down), both after Earthenware | |
| Mining | Copper Kit, A Silvery Ore, Rock Collector and An Ore Run (capstone), all after the lit kiln; side quests A Keen Edge (a copper tool mended at the grindstone, after Copper Kit) and A Bronze Edge (a bronze pickaxe, after First Bronze and A Keen Edge) | |

Copper is smelted only in the lit kiln: Red Metal, the copper kit and the ore run come after
it (Hammer and Anvil, from the kiln-age PR, goes before The Crucible when it lands). Crucible
Kiln waits on all three of its parts: the crucible, the ring of bricks and the charge. The
Bronze Age rite is unchanged (30 Farm quests, 4 Farm capstones). Rewards that handed out
what the route asks for (charcoal, clay) are gone, so the charge and the clay are earned; A
Kiln Load's four kiln bricks stay, by an explicit call, and the rest of the bricks are
shaped (Brick by Brick counts only those); Brick by Brick pays the Engineering The Crucible asks for until Hammer and Anvil
does.

Every Farm path's capstone comes after all of its path's quests, directly or through what
comes before them, but those marked `side: true` (met on the road, the bees and meadow,
the bow, ducks and such) and the age's rite; `content.rs`
`a_farm_capstone_comes_after_all_of_its_path` checks it. The kiln bot
(`a_new_player_follows_the_quest_book_to_the_lit_crucible_kiln`) walks the whole route,
one quest at a time, and lights the kiln about 330 server seconds in.

Saves (pre-release, accepted): a quest's finished state is kept by name, but quests whose
tasks changed count from what a save already tallied: Brick by Brick now counts bricks
crafted rather than held, The Crucible and A Pit Kiln also want theirs set down, and A
Kiln Yard is gone. A save part way along may have a step to do again.

## Fishing in the Farm book (Oct 20)

Fishing was the Farm book's thinnest path (Patient Angler and Many Kinds of Fish). Five
side quests (`side: true`) now run along it. Each one teaches where or how a fish bites,
or what a catch is for, and each pays in bread: no reward hands over an item that a home
quest's lesson teaches (a rug or a cushion would meet A Rug Underfoot, A Cosy Home or Heart
of the Home before the player made one). `Ate("tag:fish")` counts raw fish too, as no tag
names cooked fish alone:

| Quest | After | Teaches | Pays |
|---|---|---|---|
| Salt and Sardines | Patient Angler, Camp Cook | Sardines bite only in the sea: four caught and cooked (Cooking 5) | Fishing 80, 2 bread |
| Still Water Perch | Patient Angler | Perch bite in fresh water from Fishing 10: three caught | Cooking 60, 2 bread |
| Cold Water Trout | Still Water Perch, A Hearty Stew | The water's climate matters: two trout from cold fresh water (Fishing 15), cooked (Cooking 15) | Fishing 120, 4 bread |
| Three Waters | Salt and Sardines, Still Water Perch | A fourth kind past minnow, sardine and perch: a trout or a cod | Fishing 150, 4 bread |
| Fish on the Table | Salt and Sardines | A lit hearth, eight fish cooked and six eaten | Cooking 120, 4 bread |

None of them is in a capstone's `after`, so Many Kinds of Fish still closes the path.

## Crafting in the Farm book (Oct 20)

The Farm book's Crafting path had only its capstone, Master and Journeyman. Four side
quests (`side: true`) now teach what the bench makes from what the Farm age brings: trophies,
cloth and dyes. Each pays in bread:

| Quest | After | Teaches | Pays |
|---|---|---|---|
| Fang and Stick | Earthenware | A fang (wolves, lynx, bears) and a stick make a fang dagger (Crafting 10) | Crafting 60, 2 bread |
| Lanterns of Cloth | A Rug Underfoot | Paper lanterns (Crafting 6): two made and set down at home, each a light | Crafting 60, 2 bread |
| A Dooryard | Lanterns of Cloth | An awning and bunting made, then the Dooryard felt (an awning and two yard things under open sky) | Crafting 100, 4 bread |
| Five Colours | A Rug Underfoot, A Kiln Load | Five dyed cloths (indigo, white, red, green, amber) and the wind flags they make | Crafting 120, 4 bread |

## About camp in the Nomad book (Oct 20)

Four side quests (`side: true`) join the Nomad book's first path, each teaching a first tool
or a way to shape what is built, and each paying bread:

| Quest | After | Teaches | Pays |
|---|---|---|---|
| Flint First | Crafting Table | The flint pickaxe and the digging stick, the tools before stone | Engineering 20, 1 bread |
| A Coin Purse | Twine and Thong | A hide and two twine hold a thousand coins in one slot | Crafting 20, 1 bread |
| Steps and Slabs | A Chopping Block, A Bench to Work At | Slabs and stairs made, two of each set down | Construction 30, 1 bread |
| A Fence and a Gate | Steps and Slabs, A Bench to Work At | Posts, fences and a gate: a fence is two high to creatures; the gate swings open | Construction 40, 2 bread |

## Fire, food and night in the Nomad book (Oct 20)

Three side quests (`side: true`) on the way to the lit Homestead Hearth teach what a fire, a
meal and a home at night give, each by the buff it names (`Felt`), so they hold whatever
the buff comes to need. They sit on the cooking and home paths: the main line's walk
(`the_main_quest_line_earns_every_level_it_asks_for`) counts no buffs. Each pays bread:

| Quest | Path | After | Teaches | Pays |
|---|---|---|---|---|
| A Hot Meal | Cooking | First Night | Eating when hurt; cooked meat leaves you Hearty (`hearty`) | Cooking 20, 1 bread |
| By the Fire | Home | First Night, Crafting Table | A stool made, and sitting by the fire (`fireside`): heal half as fast again, warmed | Construction 20, 1 bread |
| An Evening In | Home | Homestead Hearth | After dark, under the roof within four blocks of the lit hearth (`evening_in`) | Construction 40, 2 bread |

A Fence and a Gate counts `plain_fence`, the straight length alone, so gates and corners
(which carry `fence` too) are not its six fences; `content.rs`
`a_fence_and_a_gate_wants_fences_not_gates` holds it.

## Readable to a newcomer (Oct 13)

Every Nomad and Farm quest was read as someone who has never played would: does its text
say what to make, where to find it, and which tool or station to use, and can everything
it asks for be come by by then? `early_quests.rs`
`every_early_quest_asks_for_what_the_land_and_recipes_give` now checks the second half on
every build: it walks from what the land, crops, catches and animals give through every
crafting and cooking recipe (a station counts only once it can be made) and fails on any
item a Nomad or Farm quest asks for that nothing reaches. Run it with
`VOXMAR_QUEST_REACH=1 -- --nocapture` for each asked item's cheapest way and level.

| Quest | Gap | Fix |
| --- | --- | --- |
| A Place to Pray | Nothing gave stone (stone breaks to cobblestone), so stone bricks, and the altar, could not be made; the text gave the wrong shape | Cobblestone fires to stone in the pit kiln, loaded and left as clay is (`stations.ron`, Construction 1). The quest comes after A Pit Kiln and says the chain: cobblestone fired to stone, four stone to four bricks, seven bricks to the altar in its real shape |
| A Pit Kiln | Said five cobblestones; the recipe takes four | Four, and where each goes |
| Workshop and Stove | Said six cobblestones for the stove (it takes five); no recipe for the quern, spindle or loom | Five, and each workshop piece's recipe and level |
| Ducks on the Pond | "Hold its favourite food" named no food | Wheat seeds or wheat; rivers and wetlands |
| The Milk Pail | Did not say how to tame a goat, reindeer or aurochs | Each one's food: wheat or an apple, a mushroom, wheat |
| Copper Arms | No recipe for the copper shield | Six ingots round a plank |
| Earthenware | No recipe for any piece | Plates, bowls, jars, jugs and the basket, with levels |
| A Friend in the Wild, Hens in a Row | Wheat seeds with no source | Break wild wheat (or your own field) for them |
| A Seat by the Fire, A Proper Bed | Furniture comes in oak, birch and dark oak only; a player with spruce planks was stuck | Both name the three woods, and the crafting table |
| First Bronze | "Put the blend in the crucible" | Hold the blend and right-click the lit kiln's crucible; four blend from one craft |
| Food Enough | Rye with no source | Rye seed comes from village fields |
| The Potter, Copper Mail | No station | At the crafting table, with the shape or level |
| Torchlight | Did not say what breaks coal | Any pickaxe, even a wooden one |

The Crucible Kiln is now three parts, as Homestead Hearth is: the crucible made, the kiln
standing (`Built`, counted when its crucible is right-clicked whole, which also names the
fuel it wants), and the kiln lit (`Lit`). Lighting always counts both, so a kiln lit
before this still reads done, and `quests_done` keeps finished quests finished.

## Every skill in the first two ages (Oct 13)

Each of the fourteen skills, the first Nomad or Farm quest that teaches it, what its early
levels open (core's `skills.ron` guide), and its first tool or station upgrade.

| Skill | First quest that teaches it | What it opens early | First upgrade |
| --- | --- | --- | --- |
| Farming | A Handful of Grain (Nomad) | Sow wheat at 1, mallards 3, carrots and mouflon 5 | Wooden to stone hoe (Break Ground) |
| Hunter | A Friend in the Wild (Nomad) | Tame fowl and rabbits at 1, mallards 3, mouflon 5, goats 8 | A knife, for more hide (A Sharp Edge) |
| Mining | A Pile of Stone (Nomad) | Stone and coal at 1, copper 5, tin 10 | Wooden to stone pickaxe (Stone in Hand), then copper (Red Metal) |
| Cooking | First Night (Nomad) | Campfire meat and fish at 1, carrots 2, mutton 5, seed mix 8 | Campfire to cottage stove (Workshop and Stove) |
| Engineering | Crafting Table (Nomad) | Tools at 1, copper ingots and tools 5, crucible 10, bronze 12 | The crafting table, then the Crucible Kiln |
| Crafting | Look It Up and Twine and Thong (Nomad) | Pouches 3, kiln bricks and workshop pieces 5, loom 8 | The pit kiln (A Pit Kiln) |
| Construction | Homestead Hearth (Nomad) | Stool 1, table 3, bed and altar 5, bookshelf 8 | A workbench at home (A Bench to Work At) |
| Fishing | Rod and Line (Nomad) | Minnows at 1, sardines 5, perch 10 | The fishing rod |
| Attack | Something to Swing (Nomad) | Stone weapons at 1, copper 5 | Fist to stone sword or bone club, then copper (Copper Arms) |
| Defence | Hide Wraps, then **Hard to Put Down** (Nomad, new) | Hide armour at 1, copper armour 10 | Hide wraps, then a shield (Raise Your Guard) |
| Hitpoints | **Hard to Put Down** (Nomad, new) | Twice your level in health; 12 is 24 | None: it grows with blows landed |
| Magic | A Place to Pray (Farm) | Bark Skin at 1, Ember Arm 4, Mending 10 | The stone altar |
| Alchemy | **Herb Lore** (Nomad, new), then Simples, Calming Herbs and Where Roots Grow (Farm) | Roots shown and an aether mote at 5 | None yet |
| Ranged | **A Stone's Throw** (Nomad, new), then **Bow and Arrow** (Farm, new) | Sling at 1, short bow 3 | Sling to short bow |

Two new side quests fill the skills that had nothing before:
- **Hard to Put Down** (Nomad, combat, after Hide Wraps) asks for Defence 4 and Hitpoints 12. Both
  skills train only in a fight, and nothing had pointed at either of them.
- **Herb Lore** (Nomad, farming, after Wild Harvest) asks the player to deal with a herb
  gatherer, a travelling alchemist or a wandering healer. These are the three wanderers that
  teach Alchemy for herbs, and they are the only Alchemy anything gives today.

Alchemy trains on nothing made before brewing, a later age's, so only quests and a
wanderer's once-only lesson raise it, and the lines it opens at Alchemy 5 (the herb gatherer
shows where roots grow, the hedge witch trades a mote of aether) were out of every early
player's reach (Oct 21). Three Farm side quests on the Fields path now take a player there,
on the 2x book scale: **Simples** (after Herb Garden: six healing herbs and four sleeping
ones picked, 200 Alchemy), **Calming Herbs** (six calming herbs picked, 200 Alchemy and two
bread) and **Where Roots Grow** (`Dealt`: the herb gatherer's roots line or the hedge witch's
mote, now at Alchemy 5). Each asks what no other quest does: meeting the healers is Herb
Lore's lesson, hanging herbs A Herb Nook's, a herb pouch Pockets'. `early_quests.rs` holds
it: the lowest Alchemy any wanderer's line asks is surely had by a quest that sends the
player to that line.

No goal opens done (Oct 21): goals count over a life, so a goal asking no more than a quest
surely done before it already asked is met on opening, and a task of such goals opens done
(Fuel to Hand's coal way after Coal and Cinders did). `no_early_goal_is_met_as_its_quest_opens`
checks every counted goal kind, not only `Level`; Fuel to Hand now asks 18 coal in all (Charcoal's coal way asks 24), and
ten goals the chain had always met went (Seed Mix's wheat, A Silvery Ore's copper, Food
Enough's wheat, An Ore Run's coal, Fired Whole's clay, The Last Sheaf's wheat, Mice in the
Stores' larder, Catch Up the Shrine's shrine bearer, A Varied Table's meat, A Stone's Throw's
kills). `no_two_early_quests_teach_the_same_lesson` holds each quest's first way (its lesson)
its own. The check reads each way of the chain too: no way opens done for a player
who took a way before it asking all it asks (Charcoal's coal way after Fuel to Hand's at 24). A
single goal goes only when every earlier way did it; one some ways did is the step for those
who took another (Found and Thanked's lost child, after A Kind Turn's child or hunter). A chain way that builds another quest's first home (Snug for Winter
taking a homely home) is that quest's lesson taken early, not a repeat; only `Built` ways
are read so.

Wanderers who trade a step of a craft had no quest to send a player to them (Oct 27). Four
side quests now do, each by the one line that trades it (`Dealt`): **Four for Twine** (Nomad,
after A Fish Supper: the fisher folk's four fish for three twine), **The Knappers' Trade**
(Nomad mining, after Stone in Hand and Pockets: a hide for flint), **A Prospector's Eye** (Farm
mining, after Charcoal: at Mining 5 the prospector points to an ore sample) and **Supper on the
Road** (Farm cooking, after Camp Cook: the travelling cook roasts one of two raw at their
fire). `every_early_dealt_line_is_open_to_the_player_it_sends` holds that a line a quest names
has no `Below` gate (a player past it would never hear it) and its `Skill` gates are surely had
when the quest opens, or asked beside it by a `Level` goal.

Coin had no lesson (Oct 27): wanderers buy goods and sell services for gold, and no quest said
so. Two Farm side quests on the callers path do: **Coin at the Roadside** (after Neighbourly:
sell once to a tanner, woodcarver, potter, shearers or ore assayer, each by their `buy` line)
and **Coin Well Spent** (pay a travelling smith to whet a blade, or a truffle hunter to point
to a truffle). The Dealt-line check also holds that what a named line takes is something the
land and recipes give.

More wanderers with no quest got one (Oct 27), each by its trading line: **Seed for Seed**
(Farm, after A Field of Gold: the seed swappers' rye for wheat seed, `swap` from Farming 3 or
`swap_more` from Farming 7, whichever the player's level reaches), **A Basket of Mushrooms**
(Nomad, after Wild Harvest: four mushrooms for a loaf), **Fangs for the Watch** (Farm, after
Bigger Game: a guard patrol's gold coin for a fang) and **Wassail** (Farm, after A Kitchen
Garden: a loaf for the wassailers' toast that grows the crops). The Dealt-line check now asks
one way of a quest to have every skill gate met (a player past one line's turn takes the
next), and checks a tag a line wants against what can be come by.

Three more got one: **Paying Respects** (Farm, after A Place to Pray: stand with a funeral
procession for their prayer), **Join the Dance** (Nomad, after Hard to Put Down: a round with
a dance circle) and **The Scarecrow's Pockets** (Farm, after Seed Mix: the seed a scarecrow
carries). The Dealt-line check now also reports, without failing, an alternative way whose
skill gate the books never surely reach, the same way the gap check reports one.

Four more: **A Bone for the Dog** (Nomad, after Small Game: a bone for a dog out walking, for
twine), **Moths to the Flame** (Nomad, after Torchlight: a torch held up for the moth
netters), **A Sweep's Luck** (Nomad, after Homestead Hearth: shake a chimney sweep's hand)
and **A Tale by Night** (Farm, after A Place to Pray: a storyteller's tale of something
strange near). The Scarecrow's Pockets now says how to reach it (it stands still while
watched), and a server test walks a watching player up to it until the trade fires The Dealt-line check
now also holds a line or party that comes only to a player with a home to quests whose
books surely make one (one only for a player without a home is shut, like a below-level gate), and a line wanting something held to a tag
something given carries.

Four more: **A Pilgrims' Blessing** (Nomad, after Folk of the Road), **The Fox and the Hens**
(Nomad, after Something to Swing: turn the fox with a blow and the farmer pays in eggs),
**The Sleepwalker** (Nomad, after Homestead Hearth: wake a sleepwalking neighbour) and **The
Hermit's Riddle** (Farm, after A Place to Pray: bring the hermit what it asks).

Ranged had no weapon at all; it now has two, and a quest for each (see "First ranged
weapons" below).

Left for later, on purpose:
- **Alchemy past a taste.** Alchemy 5 and up is brewing: tonics, then cauldrons and draughts.
  None of it exists yet, so a quest for an Alchemy level would be grinding the same three
  wanderers. Alchemy quests wait for brewing.

## A wait always has something beside it (Oct 13)

The main line has no idle waits: in the bot's timeline (docs/first-days.md), every stretch
is digging, firing or crafting. The idle waits are off it: crops growing, animals laying or
regrowing, a herb growing back, and the night. Each quest that sets one off now names, in
en and ja, an optional quest that is already open at that moment:

`a_quest_names_only_quests_open_beside_it` (crates/engine/tests/early_quests.rs) holds every
quest to this: a quest title its words name in brackets is done already or opens with what
that quest needs.

| Wait | Length | Quest that sets it off | What its text offers meanwhile (all open by then) |
| --- | --- | --- | --- |
| Wheat ripening | 12 min | First Harvest | Patient Angler, Hens in a Row, A Table for Two |
| A field resown | 12 min a crop | A Field of Gold | Camp Cook, Herb Garden |
| Carrots ripening | 14 min | Carrot Patch | A Kitchen Garden, which grows on the same crops |
| A hen laying | 5 min an egg | Fresh Eggs | A Woolly Flock, A Full House |
| Wool growing back | 6 min | Wool Gathering | A Rug Underfoot, The Milk Pail |
| Milk | 5 min | The Milk Pail | A Full Pen |
| A herb growing back | Its regrowth | Herb Garden | A Kitchen Garden (opens with it) |
| Night, which cannot be slept away | A third of each 20-minute day | Torchlight | Digging coal by torchlight; Meat on the Fire and crafting at home |
| Long clay firing | 17 + 55 s of firing for the bot (was 68 + 80) | A Kiln Load | Earthenware, A Place to Pray (its stone fires in the same kiln), Fired Whole |
| A pit kiln firing | 12 s a load | Fire the Clay | Digging the next load, tending the charcoal clamp |
| A charcoal clamp smouldering | 2 game hours (about 150 s of the quest-book bot's time, which runs about 75 s a game hour) | Charcoal | Brick by Brick: firing clay at the pit kiln to A Kiln Load's forty and crafting kiln bricks; A Second Clamp |
| A drying rack drying | 10 min (5 by a fire) | Hung to Dry | Hunting for the next load; Trail Rations |
| A compost heap rotting | 15 min (10 by water) | A Compost Heap | Sowing and tending the field it will feed, the compost's next part |

A script checked that every quest title written in parentheses in a why is a real quest's
title, and every 「」 name in ja is a real ja title, apart from the Prayers menu.

## First ranged weapons (Oct 13)

Ranged was the one combat skill with no early taste. Ages 1-2 now have two weapons, both
in content data (core/combat.ron classes and weapons, base/blocks_weapons.ron,
base/recipes_weapons.ron), checked at load:

| Weapon | Age | Level | Max hit | Cooldown | Reach | Ammo | Recipe |
|---|---|---|---|---|---|---|---|
| Sling | Nomad | Ranged 1 | 2 | 0.8 s | 12 | Any cobbled stone (`tag:cobbled`) | Twine, hide, twine (Crafting 1) |
| Short Bow | Nomad (late) | Ranged 3 | 4 | 1.0 s | 20 | Arrows | Three sticks, three twine (Crafting 3) |
| Arrow (x4) | Nomad (late) | - | - | - | - | - | Flint or cobblestone, stick, feather (Crafting 3) |

Next to early melee: fists hit 1, a stone sword 3, copper swords 5 and a copper axe 6. The
sling (2) is a step above fists that keeps you out of reach; the bow (4) sits just under
copper melee, paid for in arrows and the two hands it needs.

How a shot works, the same way every blow does:
- The server decides every hit. A shot is a swing with a class whose `ammo` is set: the
  rhythm, accuracy roll, damage and flinch are the melee ones. No knockback.
- Reach is the class's reach, and the server also needs a clear line from the eye to the
  target's centre. A wall in between drops the shot.
- Each shot uses one ammo when it lands, hit or miss. With none, the server sends "Nothing
  to shoot" and starts no swing.
- A shot that hits may drop at the target to pick up again, by the class's `recover`
  (a sling stone one time in four, an arrow one in two). A miss is lost.
- Ranged XP comes from hits, like every style skill (40 per damage, plus Hitpoints).

Quests that teach it:
- **A Stone's Throw** (Nomad, combat, after Twine and Thong): craft a sling, reach Ranged 2.
- **Bow and Arrow** (Farm, combat, after Homestead Hearth): craft a short bow and eight
  arrows, reach Ranged 5.

## The Nomad book in eight paths (Oct 20)

The Nomad chapter is `base/quests_nomad.ron` (read after `quests.ron`, docs above), drawn
as eight paths, each four to seven quests deep and ending in one capstone. Four of the old
twelve paths folded into others: Crafting into First Steps, Comforts into Home, Farming into
Food (was Cooking) and Fishing into Hunting and Fishing. Quest names, and so saves and
language keys, are unchanged.

| Path | Quests | Capstone |
|---|---|---|
| First Steps | Timber, Crafting Table, An Axe to It, A Chopping Block, A Sharp Edge, Twine and Thong | A Full Stone Kit |
| The Hearth | Look It Up, First Night, Stone in Hand, A Bed of Your Own, Torchlight | Homestead Hearth (then the rite, The Farm Age) |
| Home | Make Camp, By Rushlight, A Lit Corner, A Bench to Work At, Pockets, Company at Camp, Pantry Shelves, Put It Away | From the Shelves |
| Food | Meat on the Fire, A Handful of Grain, Daily Bread, Wild Harvest, Herb Lore, Break Ground, Tools for Animals, Hung to Dry, Trail Rations | A Varied Table |
| Hunting and Fishing | A Tempting Bite, A Friend in the Wild, Small Game, Hide and Seek, Set a Snare, Rod and Line, First Catch, A Fish Supper | Old Tusk |
| Combat | Something to Swing, Sharpening Up, Something Hostile, A Stone's Throw, Hide Wraps, Raise Your Guard | Hard to Put Down |
| Mining | A Pile of Stone, Dig In, Black Flecks, Reading the Ground, Coal and Cinders, Following the Seam, Red Metal Underfoot, Stockpile | The First Mine |
| The Wilds | Where to Settle, Wild Neighbours, Lay of the Land, Folk of the Road, A Kind Turn, Found and Thanked, Something for Something, The Market Road, Caught in the Rain, Under Lock and Key, Light Sleepers | Coal by Road |

The Mining path gains three quests on the coal ladder (piece 7's second half): Reading the
Ground (three ore samples of any kind, after Black Flecks: the first over each vein teaches
prospecting), Following the Seam (32 coal ore and Mining 8, after Coal and Cinders) and The
First Mine (48 coal ore, 12 copper ore and Mining 12, after Following the Seam and
Stockpile). Stockpile moved from First Steps to Mining, where it already followed Red Metal
Underfoot. Red Metal Underfoot now says copper wants a stone pickaxe.
