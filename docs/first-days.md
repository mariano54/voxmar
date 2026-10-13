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
