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
7. **Prayers and altars, the quern and stove are not built.** No quest asks for them.
8. **Bedding is crafted but never used**: Sleep sets the respawn point only; no Rested
   effect.
9. **Clay was only mentioned by Farm quests** though the Hearth needs it: the Hearth quest
   now says where to dig it.
10. **The First Night reward never arrived in Codex's last live gate run.** The server side
    passes in `first_night_needs_a_crafted_campfire_and_food_cooked_on_it` (same craft, cook
    and reward path), so the cause is in the gate step. The step now waits for the crafted
    campfire before cooking and its failure line says whether food or the quest is missing
    (with the quest's goal counts). The Mac run will show which.

Next (Mac): run `tools/gate` (the cloud cannot build the client's graphics stack and has
no GPU), then play the first three days once in a fresh survival world and note where you
stalled.
