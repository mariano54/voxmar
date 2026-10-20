# The first days: Nomad and Farm

Oct 13, 2026. The first two ages are **Nomad** (age 1, ends when the Homestead Hearth is lit)
and **Farm** (age 2, ends when the Crucible Kiln is lit and Bronze opens). Mariano's ask:
at least thirty quests an age, an idea of the loop in the first few days, and a check that
a new player can tell what to do from the quest text, the recipes and play. Nomad has 35
quests and Farm 44 (`assets/content/base/quests.ron`; `crates/engine/tests/early_quests.rs`
keeps the floor at 30 and checks every name they use exists).

What exists is what is written here; designed-but-unbuilt things (prayers, altars, the quern,
locked age chests, a sling) have no quest.

## The loop, day by day

A new player starts with nothing but the coin purse (`starter_kit` is empty in
`core/survival.ron`; a mod can list a kit there). The first quests teach the way in: punch
trees (Timber is pinned first), make planks in the grid, a crafting table, sticks, then a
wooden pickaxe and axe at the table, then stone. Recipes are never locked by quests; a quest only opens its
own page, and finishing is retroactive (it counts what you already did).

**Day 1, daylight: tools and a fire.** Four logs by hand (Timber), a crafting table, a
wooden axe for the rest of the wood (An Axe to It), cobblestone (A Pile of Stone), then
stone pickaxe and axe (Stone in Hand). Hunt a rabbit or hen, make a
campfire (two logs over two cobblestone in the 2x2 grid) and cook it (First Night). Wild wheat
and herbs on the way (A Handful of Grain, Wild Harvest). The quest book is the guide: Look It
Up teaches the recipe search in its first line.

**Day 1, dusk: the first night.** Torches (Torchlight), the fire, a sleeping bag (A Bed of Your
Own). Monsters are capped low on night one (`first_night_monsters`). Something to Swing and
Small Game give a weapon for the morning.

**Day 2: choose where to live.** Gather hide (Hide and Seek), fish (Twine and Thong, Rod and
Line, First Catch), and tame a hen or rabbit. Clay (four) comes from a pond bed with a shovel;
then craft the Hearth (about 28 cobblestone) and light it. **Homestead Hearth ends Nomad**:
Farm quests open. It asks only for a bed (A Bed of Your Own) and a lit Hearth, so a player who
finds stone and clay beside the camp is never held back by where they have walked. Walking
three biomes (Where to Settle, then Lay of the Land for six) is its own exploration quest,
paying Farming, Mining and Fishing experience and two bread.

**Days 3 to 5, Farm: sustain yourself, then fire.** Plant wheat (First Harvest), keep hens
(Hens in a Row, Fresh Eggs), cook (Camp Cook, level 5), a flock (A Woolly Flock). The main line
is clay work: Clay Banks, A Pit Kiln, Fire the Clay (twelve fired clay), The Potter, Brick by
Brick (nineteen kiln bricks, about 40 fired clay: four make two), and Red Metal for the copper the crucible
needs. The Crucible then The Crucible Kiln: **lighting it ends Farm**. Everything else
(husbandry, hunting, mail, home furnishings, ores, exploring) is optional breadth with its
own rewards, so a player can always see two or three things worth doing.

## Can a new player tell what to do?

Mostly yes, with these checks:

- **Every main step names its recipe.** The `why` of First Night, Stone in Hand, Hearth,
  Pit Kiln, Potter and Crucible gives the exact grid or counts; the recipe viewer (Look It Up)
  fills the rest. Each why now says where the material comes from (clay from pond beds,
  wheat for twine), not just what to craft.
- **No dead ends**: every Goal and reward name is checked against the content by the data
  test; every `after` exists; the main-line level walk (`content.rs`) still passes.
- **Sizes are honest**: the Hearth needs about 28 cobblestone and 4 clay; the Kiln about
  91 fired clay between the Potter and the Kiln. That is a long clay session; Clay Banks and
  Fire the Clay split it into steps with rewards so it reads as progress.

## Gaps found, and where each stands

1. **Nothing said "open the quest book".** Fixed: a first-time tip names the bound key
   (`tip.quests`), and a new player's book starts with Look It Up and First Night pinned
   (`tracker: (start_pins: [...])` in `quests.ron`; pinned once per install, only when nothing is done or
   pinned, so unpinning is respected).
2. **First Night counted any cooking**, so charcoal, fired clay or an ingot satisfied it.
   Fixed (Codex's contract, ported): it needs a campfire crafted and food cooked at a
   campfire (`CookAt`, tallied by station and item).
3. **The Kiln needed no fuel or light action.** Fixed (Codex's contract, ported): lighting
   needs a standing kiln and a full charge of 32 charcoal or 24 coal in hand (`Lit`, fuels
   in `multiblocks.ron`), and smelting needs a live fire.
4. **Food Enough (design F01) was absent.** Added as a Farm quest with four routes: wheat
   plus carrots or rye harvested, four animals kept at condition 75 or better
   (`Thriving`; a newly tamed animal starts at 60, so it takes a day of food, water, room
   and shelter), or forty fish. The condition goal counts what is kept now, not a streak.
5. **The Ranged guide advertised a sling that does not exist.** Removed; no bow or other
   ranged weapon exists either, so the rest of that guide is still all future.
6. **Worldgen never places the locked age chests**, so no quest depends on them.
7. **The quern and stove are not built.** No quest asks for them. Prayers and the altar
   came later and have their quests (Altar Rites, A Prayer in the Fight).
8. **Bedding is crafted but never used**: fixed by [home.md](home.md): sleeping gives a rest, and a bed, light and hearth start a ladder of home comfort.
9. **Clay was only mentioned by Farm quests** though the Hearth needs it: the Hearth quest
   now says where to dig it.
10. **The First Night reward never arrived in Codex's last live gate run.** The server side
    passes in `first_night_needs_a_crafted_campfire_and_food_cooked_on_it` (same craft, cook
    and reward path), so the cause is in the gate step. The step now waits for the crafted
    campfire before cooking and its failure line says whether food or the quest is missing
    (with the quest's goal counts). The Mac run will show which.

11. **Balance defaults** (measured below): burning and cracking start at a quarter at level
    1 and reach nothing by the mid-teens of the skill (a cracked clay is clay again); the
    level-1 sleeping bag is two feathers and two hides, so a new player has a bed on day
    one; Homestead Hearth no longer waits for three biomes (Where to Settle is its own
    exploration quest).

12. **Finding animals took no effort** (playtest): they were found from 16 blocks off,
    through walls. Now an animal is found only within 3 blocks with nothing in between
    (`seen_within` in `core/journal.ron`), for every quest that counts found animals.
13. **Wild Harvest counted any five picks** (playtest), so one flower bed did it. It now
    asks for five different plants (`Kinds(Plants, "any", 5)`, a general goal for N
    different entries of a journal page).

14. **Nothing said where things are, and wild wheat was nearly absent** (playtest: "no
    help about where to find wheat"). A census (`rare_herb_census` with `WILD=wild_wheat`,
    seeds 7 and 1) found about one tuft per 30,000 columns on plains: it grew as a flower
    drift, so it waited on blooms and was thinned with the flowers. It now grows alone
    and is named by the grassland biomes (`plants.ron`, `biomes.ron`): one tuft in about
    400 columns on plains, 480 on steppe, 550 on meadows, 920 in flower meadows. Every
    gather and find step's quest text now says where its thing is found, checked against
    the biomes, creature habitats, ore veins and that census (wild wheat on grassland and
    in village fields; copper and coal under their ore samples; tin below copper; herbs,
    berries and flowers by biome; each animal by its habitat).

15. **Steps asked for things the text never showed how to make** (Oct 13 walk of every
    Nomad quest in order, then Farm up to the lit Crucible Kiln, checking each ask against
    `recipes.ron`, skill unlocks and where things are found). Fixed in the quest text:
    Crafting Table names its four planks; Stone in Hand says a wooden pickaxe breaks stone
    into cobblestone; Torchlight gives coal and a stick and where coal shows; Pockets names
    hide and twine and Crafting 3; First Ore says Mining 5 comes from coal and now follows
    Coal and Cinders; A Friend in the Wild says where wild carrots grow; Break Ground and
    First Night say where cobblestone comes from; Tools for Animals names the wild herds
    (mouflon, mountain goats, reindeer, aurochs) instead of farm animals the world lacks;
    Red Metal, Copper Kit and The Crucible give the ingot recipe (copper ore over coal or
    charcoal, Engineering 5) and where Engineering comes from; Crucible Kiln gives the
    layout (a ring of eight kiln bricks and four corner pillars) and the charge; The Potter
    names Crafting 2; A Woolly Flock says wheat is the favourite food. Checked and right as
    written: the hearth and chimney, sleeping bag, bread, shears, pail, hide armour,
    buckler, knives, shovel, rod, pit kiln, kiln bricks, crucible, clay pot, charcoal at a
    campfire and tin at Mining 10.

Next (Mac): run `tools/gate` (the cloud cannot build the client's graphics stack and has
no GPU), then play the first three days once in a fresh survival world and note where you
stalled.

## The new-player bot

`a_new_player_follows_the_quest_book_to_the_lit_crucible_kiln` (crates/server/tests/gameplay.rs)
runs in every test run and guards the loop. A scripted player starts with nothing and follows
the main line in the book's order: Look It Up, the Nomad quests to the lit Homestead Hearth,
then A Pit Kiln, Fire the Clay, The Potter, Charcoal, Red Metal, Brick by Brick, The Crucible
and the lit Crucible Kiln. It gathers, crafts with the recipes it has unlocked, fires clay,
burns charcoal, hunts, and claims each quest's rewards in the book as it finishes, since the
levels later steps need come from those (Red Metal's copper takes Mining 5, and all the
mining before it gives Mining 2; Charcoal's reward brings the rest). Nothing is given: the plot
is laid out with /fill and /summon, standing in for terrain, and the only other shortcut is
a /tp across the plot. Each quest must show done at its step, so a step whose text and
recipes cannot be followed fails here.

Its messages are stamped with the game's clock (`Fresh::now`), so mining, cooking, walking and
blows go at the test server's ticks, and the server runs in lockstep with the bot
(`ServerConfig::lockstep`): it ticks only once the bot has taken in the tick before, as quick
as the bot goes. The run takes about 4 s of wall time for some seven minutes of game time,
and a bot starved of the cores on a busy machine loses no ticks: the game time is the bot's
own however loaded the machine. For that, a test server's swings land by the
latest stamp heard when it is later than the clock (`tick_fights`), which over a network it
never is.

### The bot's timeline

The bot prints a timeline (`timeline` in gameplay.rs, `--nocapture`): each milestone's server
seconds, what the phase before it went on, the levels after it, and the side quests the way
finished, which it claims as a player opening the book would. It plays at its own
efficient pace and knows the way, so a new player takes several times longer; the shape
is what counts. Medians of three runs on 2026-10-13, with the range:

| Milestone | Server s | Range | What the phase went on |
|---|---|---|---|
| Look It Up | 2 | 2 | The recipe viewer. |
| Timber, Crafting Table | 41 | 40 to 42 | Sixteen logs punched by hand (35 s). |
| Stone in Hand | 53 | 53 to 57 | Nine cobblestone with the wooden pickaxe. |
| First Night | 73 | 66 to 73 | Hunting in the lane, two seconds a cooking. |
| A Bed of Your Own, Torchlight | 85 | 77 to 90 | Two feathers and two hides; two coal. |
| Homestead Hearth lit | 118 | 106 to 119 | 28 cobblestone and 4 clay (A Pile of Stone and Dig In fall on the way; Make Camp with the hearth). |
| A Pit Kiln | 122 | 113 to 127 | Seven cobblestone, a campfire. |
| Fire the Clay | 175 | 171 to 180 | 28 clay dug (Clay Banks on the way), twelve fired with cracks at Crafting 1 to 3. |
| The Potter | 183 | 179 to 194 | Three more fired, three crafts. |
| Charcoal | 260 | 258 to 268 | 52 logs chopped for all the charcoal ahead (33 s), sixteen burnt (35 s). |
| Red Metal | 271 | 269 to 279 | Nine copper ores, three ingots, the pickaxe. Charcoal's claimed reward takes Mining from 3 to 5 and more. |
| A Kiln Load | 375 | 371 to 379 | Firing on to forty in all (59 s of firing). |
| Brick by Brick | 478 | 468 to 484 | Sixty more fired for fifteen bricks (97 s of firing). |
| The Crucible, Crucible Kiln lit | 489 | 479 to 495 | The crucible, twelve bricks set, three logs burnt to make up the charge. |

Where the grind was, and what changed (Oct 13, from the bot's first timeline):

- **Brick by Brick was 216 s of the 540, 36% of the whole line**, with no reward from
  Red Metal to the end: 76 fired clay at two seconds each and some cracks. A Kiln Load
  (pottery, after The Potter) now falls halfway, at forty clay fired in all, and pays
  four kiln bricks; Brick by Brick asks for nineteen bricks in hand, so the reward
  counts. The stretch is now two halves of 95 and 113 s with a reward between.
- **The kiln's charge was another 44 s of burning logs** after the crucible. The Crucible
  now pays sixteen charcoal, half the charge, and its text says so: the reward teaches
  the next step. That stretch is now 9 s.
- **Mining 2 to 5 before Red Metal.** All the mining on the way gives Mining 3; Charcoal's
  reward brings the rest once claimed, as Red Metal's text now says (PR 86).
- Still long, and left as they are: punching the first logs by hand (40 s; the first tool
  needs the table) and Charcoal's 75 s, which chops the wood for every later charge too.
  The whole line went from 540 to 489 server seconds.
- **A full pack.** Cooked food takes a slot each (core's `stacking.ron`, as in RuneScape,
  so what a player carries into a fight is a choice), and Where to Settle and Homestead
  Hearth paid ten bread between them, so by Charcoal the bot's 36 slots were full: A Kiln
  Load's bricks, a copper ingot or dug clay then had nowhere to go (a claim or craft
  refused). Those two now pay three (two and one), and the bot still throws away its spare
  (burnt food, feathers, the wooden pickaxe, raw meat) at Charcoal if the pack fills.

### Quick upgrades on common resources

Oct 13, from the bot's timeline: the first upgrade for each resource a new player gathers
in the first two ages, when the bot reaches it, what it costs and how much faster it is.
Mining time is the block's hardness over the tool's speed (tools.ron), so a tool's
`speed` is the whole of the gain.

| Resource | Bare | First upgrade (bot reaches it) | Cost | Faster by | Next |
|---|---|---|---|---|---|
| Wood (log 2.0) | 2.0 s | Wooden axe (32 s, was never made) | 3 planks, 2 sticks | x2 | Stone axe at 44 s, x4 |
| Stone (1.5) | 1.5 s | Wooden pickaxe (13 s) | 3 planks, 2 sticks | x2 | Stone pickaxe at 44 s, x4; copper at 233 s, x7 |
| Clay (0.6) | 0.6 s | Stone shovel (Dig In, about 100 s) | 1 cobblestone, 2 sticks | x4 | Copper shovel, x7 |
| Coal ore (3.0) | none | Wooden pickaxe (13 s) | as above | needs a pickaxe | Stone pickaxe, x2 more |
| Fibre (wild wheat, grass) | instant | none needed | | | Sown wheat in the Farm age; wild wheat gives an ear 6 in 10 |
| Meat (hunting) | max hit 1 | Stone axe or sword (44 s) | 3 or 2 cobblestone, sticks | x3 max hit | Copper sword, max hit 5 |
| Charcoal (a log in the fire) | 2 s each | none: coal ore instead (Charcoal's other route) | | | The Crucible pays 16 |

What changed:
- **Wood was the late one.** Timber asked for eight logs by hand, and nothing asked for the
  wooden axe, so a player (and the bot) punched every log until the stone axe. Timber now
  asks for four, exactly the planks for the table, sticks, wooden pickaxe and axe, and
  says to chop the rest with the axe. A new side quest, An Axe to It (after Crafting
  Table), asks for the wooden axe and eight logs, pays four sticks toward Stone in Hand,
  and says the stone axe halves the time again. The first upgrade is now felt within the
  first minute: twelve logs by axe take about 19 s where punching them took about 30.
- **Hunting.** The stone axe made for Stone in Hand already hits three times a fist, but
  nothing said so. First Night's text now tells the player to swing it.
- Stone, clay and coal were already quick: the wooden pickaxe comes with the table, and
  Dig In asks for the stone shovel before the hearth's clay.
- Charcoal has no faster tier, and none was added (no new systems): coal ore is the other
  route, and The Crucible's sixteen charcoal now covers half the kiln's charge. The bot had
  also been chopping 52 logs for 20 it used; it now chops 24, which takes the Charcoal
  phase from 75 to 49 s. That is the bot's own waste, not the game's.

The bot's timeline after this (three runs, Oct 13), against the one above:

| Milestone | Before | After |
|---|---|---|
| Timber | 41 | 13 (four logs) |
| An Axe to It | | 32 to 33 |
| Stone in Hand | 53 | 44 to 50 |
| First Night | 73 | 59 to 71 |
| Homestead Hearth lit | 118 | 100 to 112 |
| Fire the Clay | 175 | 162 to 184 |
| Charcoal | 260 | 222 to 242 |
| Red Metal | 271 | 233 to 252 |
| A Kiln Load | 375 | 317 to 344 |
| Brick by Brick | 478 | 439 to 449 |
| Crucible Kiln lit | 489 | 450 to 459 |

### Pacing pass with everything landed (Oct 14)

On main cc0ccf78 (storage, stations, chests, bees, the Rootmother, ranged, the teaching
quests all landed), the bot's line was unchanged by any of them: none of them sits on the
main line, so they are side content a player picks up in the waits. Medians of five runs
on main, then twelve with the earth clamp (docs/stations.md); "at fire" is the server
seconds the bot stood at a fire or kiln in that phase.

| Milestone | Main | At fire | With the clamp | At fire |
|---|---|---|---|---|
| Homestead Hearth lit | 107 | 0 | 109 | 0 |
| Fire the Clay | 167 | 35 | 171 | 35 |
| The Potter | 176 | 6 | 185 | 6 |
| An Earth Clamp made | | | 206 (196 to 219) | 0 |
| Charcoal | 226 | 35 | 272 | 48 |
| Red Metal | 236 | 0 | 283 | 0 |
| A Kiln Load | 341 | 68 | 317 | 17 |
| Brick by Brick | 440 | 80 | 397 | 55 |
| Crucible Kiln lit | 450 (430 to 478) | 230 in all | 408 (396 to 438) | 167 in all |

The stall: from Fire the Clay to Brick by Brick, half the line (230 of 450 s) was standing
at a fire with one clay or log in hand, and from Red Metal to Brick by Brick (204 s) nothing
new came but the brick recipe. Nothing in it was a quick upgrade or a new thing to learn.

What filled it: the earth clamp, a home station of eight dirt round a log (Construction 3),
named by a new quest, An Earth Clamp (after The Potter), and by Charcoal's and Brick by
Brick's text. It fires sixteen clay (never cracked) or chars sixteen logs in a minute with
nobody by it, and quests count what it gives as cooked. It is slower than the hand, so it
is no replacement: the player keeps it smouldering while they fire by hand, dig and mine.
The line is 42 s shorter, 63 s less of it is spent at a fire, and the longest stretch with
nothing new (Red Metal to Brick by Brick) is 114 s instead of 204. Charcoal comes later (the
clamp's minute is spent firing clay toward A Kiln Load, which then comes sooner).

Still long, and left: Fire the Clay (about 60 s, 35 of it firing, before the clamp is known)
and punching the first logs. The clamp could open with A Pit Kiln to cover Fire the Clay
too; that is the next step if the first Farm minutes read as slow in play.

### Fire the Clay and the Nomad pass (Oct 14)

Medians of twelve bot runs each, before lockstep (game seconds; the earlier columns are the
clamp after The Potter, the later ones the clamp after A Pit Kiln, then A Tempting Bite too).

| Milestone | Clamp after Potter | After A Pit Kiln | And A Tempting Bite |
|---|---|---|---|
| First Night | 61.5 | 61.8 | 62.1 |
| A Tempting Bite | | | 71.3 |
| A Bed of Your Own | 68.7 | 67.6 | 78.8 |
| Homestead Hearth lit | 104.2 | 104.4 | 116.8 |
| An Earth Clamp made | 198.9 | 121.4 | 133.5 |
| Fire the Clay | 166.9 (56.1 in it, 34 firing) | 175.1 (53.8) | 187.4 (57.2) |
| A Kiln Load | 318.8 (32.5 in it) | 285.6 (0.8) | 297.9 (0.8) |
| Crucible Kiln lit | 410.8 | 376.0 | 388.6 |

Fire the Clay: the player stood at the pit kiln for most of a minute with nothing to learn.
The clamp now opens with A Pit Kiln, and Fire the Clay's text sends the player to heap one
while they wait. The stretch is no shorter (the clamp is built inside it), but it now
teaches a station, and the line to the Crucible Kiln is 35 s shorter because A Kiln Load's
wait is gone.

Nomad: the bot's longest stretch with nothing new is the Hearth's 28 cobblestone (about 25
s), which A Pile of Stone and Dig In already fill. For a real player the stall is the hunt
for A Bed of Your Own: rabbits run at 7 blocks a second, and the bot's stocked lane hides
that. A Tempting Bite (hunter, after First Night) teaches bait: hold the favourite food (a
carrot from wild carrot for rabbits, wheat seeds from wild wheat for junglefowl) and they
walk up instead of running. A new `Lured` goal counts each wild creature drawn within three
blocks once. It costs the bot about 9 s, since its lane needs no chase, and saves a player
the chase.

In lockstep (twelve runs, against the clamp after The Potter in lockstep): Homestead Hearth
lit 98.6 (was 89.8), An Earth Clamp made 112.2 (177.7), Fire the Clay 158.5 (148.2), A Kiln
Load 266.4 (269.9), Crucible Kiln lit 351.7 (341 to 357; was 361.2), and 145 s at a fire or
kiln in all (167).

In lockstep (Oct 14) the bot loses no ticks to its own turns, which a server ticking at 60
times the clock's pace ran on through, about a tenth of each step even on a calm machine
(Timber 13 to 12, An Axe to It 34 to 30). Five runs: Timber 11 to 12, An Axe to It 29 to 30,
Stone in Hand 39 to 40 (the same each run, as the rest would be but for the hunt and the
firing's cracks), First Night 52 to 60, Homestead Hearth lit 82 to 95, Fire the Clay 137 to
148, Charcoal 196 to 207, Red Metal 205 to 216, A Kiln Load 292 to 316, Brick by Brick 381 to
408, Crucible Kiln lit 392 to 418.

### First-hour pacing (Oct 14)

A pass over every Nomad quest for stretches of more than about three minutes of the same
gathering, from the bot's main line (lit hearth at 81 s of action, nothing near three
minutes) and, for side quests, drop chances, tool speeds and a census of the land around
five spawns (the table is in the project's design/first-hour-pacing.md). Wild wheat is the
one that was long: lone tufts some 25 blocks apart on grassland, none in most woods, and
three ears in ten made Daily Bread's eight wheat about 27 tufts. Foraging now gives an ear
six times in ten (`crops.ron`), about 13 tufts; sown wheat stays the bulk source
(`wild_wheat_bakes_bread_in_a_short_walk`). A Varied Table (12 meat, 8 fish, 6 bread,
about 11 minutes) and Trail Rations (12 jerky) were long by their counts, which the quest
book's owners set.

### Farm pacing (Oct 14)

The same pass over the Farm age, from the lit hearth to the lit Crucible Kiln (the table is
in the project's design/farm-pacing.md). Firing clay for the kiln was the one long stretch
of a single action: Brick by Brick's nineteen bricks were 76 fired clay, 91 s of the
quest-book bot's 394. Four fired clay now make two bricks (`recipes.ron`, the same
Crafting experience a brick), and Brick by Brick takes 16 s; the kiln is lit at 319 s.
`hunt_pace_probe` (crates/server/src/creatures.rs) measures a kill in the wild at 5 to 9 s
on level ground, and a cast lands two times in five, about 12.5 s a fish. Crops take 12
minutes of growth (wheat; carrots 14) day and night, and while they grow the main line,
husbandry and the home quests are open.

### Pacing: new things per three minutes (Oct 20)

The quest-book bot also counts what is new to the player as it goes (`Pacing` in
gameplay.rs): an item first in the pack, a place (a biome stood in, a cave, ruin or village
entered, from the journal), a creature first met (the journal's animals, monsters and
bosses), and a station first worked at (a block tagged `station`, a home station from
`stations.ron`, or a multiblock's core). It prints them for each three minutes of the run,
the longest wait with nothing new, and the longest stretch of one action over and over (the
same action on the same block, creature or held item, until the player does anything else).
It fails if a whole three minutes brings fewer than five new things (`PACING_NEW`) or one
action goes on for more than a minute (`PACING_STRETCH`).

32 runs on main 3930bb29 (server seconds):

| Measure | Range | Median | What it was |
|---|---|---|---|
| New things, first three minutes | 27 to 43 | 36 | Tools, the first foods, the fire, the first two creatures and two biomes. |
| New things, the rest | 9 to 19 | 15.5 | The bed, clay, the Hearth, the pit kiln and clamp; at least 14 when the window is a whole three minutes. |
| Longest wait with nothing new | 49 to 170 | 88 | Firing clay toward A Kiln Load while the clamp chars (Charcoal), 80 to 106 s in 31 runs; once the hunt for A Bed of Your Own's hides (170 s, 109 of it hunting). |
| Longest single-action stretch | 29 to 40 | 31 | Firing clay at the pit kiln, 13 to 17 in a row (Fire the Clay). |
| The lit Crucible Kiln | 314 to 401 | 356 | |

The target is 20 to 30 server minutes from a new world to the Farm age's end. The bot is
a floor: it knows the way, its plot stands in for terrain, and it has no menus or
reading, so 5 to 7 minutes for the bot is in line with a player several times slower.
What the report shows, and is left for the books' owners:

- **The Charcoal stretch.** From The Potter to the clamp's sixteen charcoal, about 85 s
  bring nothing new: the bot fires clay by hand while the clamp works. That is the next
  place for a new thing (a side quest or a station that opens there).
- **The hunt's tail.** The bed's two hides take 26 to 171 s; the slow runs are the chase,
  not the drop (rabbit and junglefowl hides drop 0.8). A Tempting Bite (bait) is the
  answer the book already gives; a real player is slower still.
- Nothing past the first window is a place or a creature: the plot is one or two biomes
  and a lane of two kinds. In a real world the walks add both, so the second and third
  windows are a floor too.

### One charcoal route (Oct 20)

Charcoal comes only from the charcoal clamp now: the earth clamp is retired (its blocks load
as oak logs) and the campfire no longer chars logs. The quest-book bot builds the clamp as
Charcoal says: eight logs, a flint dug from the gravel bank for the chopping block, ten
firewood split on it (nine laid), kindling, twenty-four dirt for the walls and roof, lit
from a torch. The bot's clock runs about 75 s a game hour.

At eight game hours the clamp failed the pacing check above: about 600 s of the bot's time
with nothing new and the bot idle (0 new things in three windows, one stretch of 440 s). It
smoulders two game hours now (`smoulder.hours` in multiblocks.ron), and while it does the
bot works on Brick by Brick, open since The Potter: clay fired to A Kiln Load's forty, then
kiln bricks crafted between firings. Once the clamp is spent it digs out the charcoal and
does Red Metal, then finishes the bricks. Seven runs: the lit Crucible Kiln at 465 to 520 s;
the window from 360 s brings 7 new things (kiln bricks, charcoal, copper ore, ingot and
pickaxe, the crucible and its station); the longest wait with nothing new is about 150 s
(firing clay); the longest single-action stretch is 39 to 40 s (17 firings in a row).

## Measured pacing

Run it with `cargo test -p voxmar_server --test gameplay a_fresh_survival -- --ignored --nocapture`
(about three minutes of real server time, so it is ignored in ordinary runs); run it after
changing early-age quests, recipes or cooking. It shares its script with the bot above and
claims each quest's rewards as it finishes.

`a_fresh_survival_player_reaches_each_nomad_milestone` (crates/server/tests/gameplay.rs) plays
the start of a fresh survival game headless, with only actions a player can propose: mine at
real tool speed, collect drops, craft, place, cook (2 s each), light, strike animals. Nothing
comes from a command: no /give, /skill, /xp, /clear, no /tp of the player, no fill into the
inventory. Times are server seconds (20 ticks per second, counted from the server's own tick
counter, never wall clock), measured from the first action. The test runs in real time
because mining and cooking are timed by message arrival, so it takes 3 to 4 minutes of wall
time. It asserts only the budget table at the top of the test (generous ceilings, so a
regression such as a recipe that suddenly needs ten times the work fails it) and prints one
summary block to stderr (`--nocapture`).

Medians of three runs each. "Before" is the first measurement (four runs, with the old
starting kit and before the balance defaults below); "Kit" is after the balance defaults,
still with the wooden tools, bread, torches and planks of the old kit; "Now" starts with
nothing, bare hands on the first trees. "Range" is the fastest and slowest run of "Now".

| Milestone (quest) | Before | Kit | Now | Range now | What the time went on |
|---|---|---|---|---|---|
| 8 logs (Timber) | 14.8 s | 14.8 s | 35.2 s | 35.2 | Punching 16 logs by hand (33 s; the axe made it 13), 2 s of walking between trunks. |
| Crafting table | 14.8 s | 14.8 s | 35.2 s | 35.2 | Four logs to sixteen planks, the table, sticks and a wooden pickaxe: crafting takes no server time. |
| Stone pickaxe and axe (Stone in Hand) | 23.9 s | 23.9 s | 44.3 s | 44.3 to 44.8 | 9 cobble with the wooden pickaxe (about 7 s of mining), then crafting. |
| Campfire and a cooked meal (First Night) | 66 s | 53.5 s | 76.2 s | 74 to 81 | Walking to and chasing animals, killing rabbits and junglefowl, 2 s per cook. |
| Torches and sleeping bag (A Bed of Your Own) | 89 s | 56.8 s | 84.7 s | 82 to 106 | Two feathers and two hides, crafted in the hand grid. Torches add 4 to 10 s (2 coal). |
| Hearth materials (28 cobble, 4 clay) | 113 s | 83.2 s | 110.1 s | 110 to 134 | About 15 s of mining cobble, clay from the pond bank with a stone shovel, 6 to 8 s of walking. |
| Homestead Hearth built and lit | 117 s | 86.2 s | 113.6 s | 113 to 138 | 3 s: ring of 8 cobble, hearthstone, 2 chimneys, light. Home is set ("your home"). |
| Pit kiln built | 124 s | 92.4 s | 119.7 s | 120 to 144 | 6 s: 7 more cobble, a campfire, the kiln recipe. |
| 12 fired clay (Fire the Clay) | 176 s | 145.8 s | 168.5 s | 164 to 197 | 28 clay to dig (7 s), then 31 to 40 s of cooking: about a fifth of the attempts crack, and a cracked clay comes back as clay. |

The "before" Hearth rows are when the Hearth was built and lit; the quest itself stayed held
until three biomes were walked, which the plot cannot do (see "What the run showed"). Now the
run ends with Homestead Hearth, A Pit Kiln and Fire the Clay all done, with the biomes seen at
2 of 3.

Blocks mined over the whole run: about 90. Animals killed: 7 to 8 (no restock of the lane
was ever needed). By the Hearth the player has spent roughly 35% of the time mining, 45%
walking and 20% hunting and cooking. Everything up to the Hearth is under two minutes in a
typical run, i.e. a small part of day one (a day is 24000 ticks, 1200 s); the whole
Nomad opening, including 12 fired clay, fits in about three minutes.

### Assumptions (read before trusting the numbers)

- **The player stands beside a plot built by /fill and /setblock** (trees, stone, coal, a clay
  pond, a table-sized clearing) and /summon puts 10 animals in a 1-wide lane beside it.
  These stand in for terrain. Real worlds have to be explored for them.
- **Walking between biomes is not modelled.** Real stone, clay, coal and herds can be a long
  walk away; the "walking" figures here are only the few metres inside the plot. Add your own
  estimate for finding each resource. Because of this the numbers are a floor, not a forecast.
- **Animals come by command.** No herd search, no flight, no waiting for spawns. The lane is
  stocked at the start (and would be restocked if empty).
- Mining uses real tool speed, so stone with the stone pickaxe, clay with the stone shovel
  and logs with the axe are all honest. A drop has a 0.5 s pickup delay and a 2.5 block magnet.
- The player does perfect inputs: no hesitation, menus, misclicks or reading. Real players
  will be slower, mostly on the first two minutes.

### What the run showed

- **Cooking burned half of the attempts at Cooking level 1 and the raw item was lost; now it
  burns a quarter** (`BURN_AT_START` in `engine/src/cooking.rs`, or a recipe's `burn_start`),
  falling in a straight line to nothing at `stop_burning`, the mid-teens for the first foods
  (15 for meat, fish and potatoes, 12 for eggs and clay). That was the largest source of
  variance in First Night (47 to 101 s, now 45 to 56 s).
- **Fired clay is still the slowest step**, but a cracked clay now comes back as clay
  (`burnt: "clay"`), so a crack costs two seconds, not the dig. 12 fired clay took about 20
  attempts (35 to 40 s) against about 26 (33 to 55 s) before.
- **The sleeping bag is two feathers and two hides at Crafting level 1** (the hand grid, no
  table). It was seven feathers and two hides in a three by three. Wool and hide or fur still
  need Crafting 5, so the first bed is a junglefowl and rabbit hunt, not a sheep.
- **The Homestead Hearth quest could not finish on the plot, and now can.** It had `after:
  ["a_bed_of_your_own", "where_to_settle"]` and Where to Settle needs three biomes visited;
  the plot is one, sometimes two, so the quest stayed "held, not done" and the farm quests
  chained on it (`a_pit_kiln`, `fire_the_clay`) were held too. Biome visits no longer gate
  the core chain: Homestead Hearth waits only for A Bed of Your Own. Where to Settle is an
  exploration quest of its own with a reward worth the walk.
- **Starting with nothing costs about 20 s, all of it on the first trees**: bare hands punch
  a log in about 2 s against the wooden axe's 1. After the wooden pickaxe the run keeps pace
  with the kit runs. The first night has no torches until Torchlight's coal.
