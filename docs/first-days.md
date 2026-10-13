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

The starting kit is a wooden pickaxe and axe, four bread, eight torches, sixteen planks and
a purse (`core/survival.ron`). Recipes are never locked by quests; a quest only opens its
own page, and finishing is retroactive (it counts what you already did).

**Day 1, daylight: tools and a fire.** Logs (Timber), cobblestone (A Pile of Stone), a
crafting table, then stone pickaxe and axe (Stone in Hand). Hunt a rabbit or hen, make a
campfire (two logs over two cobblestone in the 2x2 grid) and cook it (First Night). Wild wheat
and herbs on the way (A Handful of Grain, Wild Harvest). The quest book is the guide: Look It
Up teaches the recipe search in its first line.

**Day 1, dusk: the first night.** Torches (Torchlight), the fire, a sleeping bag (A Bed of Your
Own). Monsters are capped low on night one (`first_night_monsters`). Something to Swing and
Small Game give a weapon for the morning.

**Day 2: choose where to live.** Walk three biomes (Where to Settle, then Lay of the Land
for six), gather hide (Hide and Seek), fish (Twine and Thong, Rod and Line, First Catch),
and tame a hen or rabbit. Clay (four) comes from a pond bed with a shovel; then craft the
Hearth (about 28 cobblestone) and light it. **Homestead Hearth ends Nomad**: Farm quests open.

**Days 3 to 5, Farm: sustain yourself, then fire.** Plant wheat (First Harvest), keep hens
(Hens in a Row, Fresh Eggs), cook (Camp Cook, level 5), a flock (A Woolly Flock). The main line
is clay work: Clay Banks, A Pit Kiln, Fire the Clay (twelve fired clay), The Potter, Brick by
Brick (nineteen kiln bricks, about 76 fired clay), and Red Metal for the copper the crucible
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

## Gaps found (not all fixed here)

1. **Nothing says "open the quest book".** The book (L) holds the guide, but a new player is
   told nothing. Fixed here with a first-time tip (`tip.quests`, names the bound key).
2. **No auto-pin.** The tracker starts empty; a player must pin quests by hand. Left as is;
   the tip says to pin up to three.
3. **First Night counts any cooking** (`Cook("any")`): charcoal, fired clay or an ingot
   satisfies it. Codex's branch `codex/nomad-farm-first-contracts` (aadac231) tightens this and
   makes the Kiln need a real lit, fuelled run; its last live test failed on the First Night
   reward. Not merged here: it changes goal semantics and needs its own landing.
4. **The Kiln does not need fuel or a light action**: it checks the structure and tallies
   `Built` (same branch above).
5. **Worldgen never places the locked age chests**, so lockpick quests and the chest loop
   are out of the first days; no quest here depends on them.
6. **The Ranged guide advertises a sling that does not exist** (skills.ron). Left; needs the
   sling item or a text change by Combat/Gameplay.
7. **Prayers and altars, the quern and stove are not built.** No quest asks for them.
8. **Food Enough** (design F01, two crops and four animals content for a day) is absent; the
   journal cannot express sustained farming. Farming, Hens in a Row and Camp Cook stand in.
9. **Bedding is crafted but never used**: Sleep sets the respawn point only; no Rested effect.
10. **Clay was only mentioned by Farm quests** though the Hearth needs it: the Hearth quest now
    says where to dig it.

Next (Mac): run `tools/gate` (the cloud cannot build the client's graphics stack and has
no GPU), then play the first three days once in a fresh survival world and note where you
stalled.

## Measured pacing

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

Four full runs. "Typical" is the median, "range" is the fastest and slowest.

| Milestone (quest) | Typical | Range | What the time went on |
|---|---|---|---|
| 8 logs (Timber) | 14.8 s | 14.8 to 15.1 | Chopping 12 logs with the wooden axe (13 s of the 15), 2 s of walking between trunks. |
| Crafting table | 14.8 s | 14.8 to 15.1 | Free: the logs for it were already in hand. Crafting takes no server time. |
| Stone pickaxe and axe (Stone in Hand) | 23.9 s | 23.9 to 24.4 | 9 cobble with the wooden pickaxe (about 7 s of mining), then crafting. |
| Campfire and a cooked meal (First Night) | 66 s | 47 to 101 | Walking to and chasing animals, killing rabbits and junglefowl, 2 s per cook. The spread is cook burns (see below) and hunting luck. |
| Torches and sleeping bag (A Bed of Your Own) | 89 s | 66 to 115 | Hunting again for 7 feathers and 2 hides (rabbit hide drops 80%, feathers 90%), 12 s of walking. Torches add 4 s (2 coal). |
| Hearth materials (28 cobble, 4 clay) | 113 s | 90 to 139 | About 15 s of mining cobble, clay from the pond bank with a stone shovel, 6 to 8 s of walking. |
| Homestead Hearth built and lit | 117 s | 93 to 142 | 3 s: ring of 8 cobble, hearthstone, 2 chimneys, light. Home is set ("your home"). |
| Pit kiln built | 124 s | 100 to 148 | 7 s: 7 more cobble, a campfire, the kiln recipe. |
| 12 fired clay (Fire the Clay) | 176 s | 155 to 221 | 28 clay to dig (7 s), then 33 to 55 s of cooking, because half the attempts are lost (see below). |

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

- **Cooking burns 50% of the time at Cooking level 1 and the raw item is lost.** A cooked
  meal takes two attempts on average. This is the largest source of variance in
  First Night (47 to 101 s).
- **Fired clay is the slowest step of all.** 12 fired clay took about 26 cook attempts (33 to
  55 s) because clay cracks at the same 50% rate. The pit kiln tail is a third of the run.
  If the "fire the clay" step should be quicker, the crack chance is the lever, not the
  2 s cook time.
- **The sleeping bag needs 7 feathers and 2 hides at Crafting level 1.** Wool and hide or fur
  need Crafting 5, so the first bed is a junglefowl and rabbit hunt, not a sheep.
- **The Homestead Hearth quest cannot finish on the plot.** It has `after:
  ["a_bed_of_your_own", "where_to_settle"]` and Where to Settle needs three biomes visited;
  the plot is one, sometimes two. The hearth is built and lit and `player.home` is set, but
  the quest stays "held, not done". The farm quests that chain on it (`a_pit_kiln`,
  `fire_the_clay`) are then held too, even though their goals (kiln built, 12 fired clay)
  are fully met. This is the exact blocker for completing the Nomad chain without leaving the
  plot; it is a design fact (the quest is meant to be done after exploring), not a bug, but a
  player who builds the Hearth early sees no reward for it until they have wandered.
- **The starter kit leaves one hotbar slot free (slot 8)**; bread takes four slots. The test
  swaps tools through that slot.
