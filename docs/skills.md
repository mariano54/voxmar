# Skills

Oct 6, 2026. The skill list and what each level unlocks. Part of the creatures, food and
breeding design: read [systems-web.md](systems-web.md) for how the systems feed each other
and [creature-data.md](creature-data.md) for the data formats.

## Decisions

| # | Question | Decision |
| --- | --- | --- |
| 1 | Curve | RuneScape's, levels 1 to 99 (13,034,431 experience at 99, level 92 is half way), shared by every skill. Already in `core/skills.ron`; a later package's `curve` replaces it, and the load log names both packages (Design draft 91). |
| 2 | Skills | Attack, Defence, Hitpoints, Mining, Farming, Cooking (Mariano's list), plus **Hunter**, which Mariano chose on 2026-10-05; Engineering and Crafting (2026-10-06), Fishing, and Ranged, Magic, Alchemy and Construction (the progression plan, P25), whose guides list what is to come. Content peaks at 80; only Engineering runs to 90. |
| 3 | Breeding | Part of Farming, never its own skill (Mariano). Farming covers crops, trees, beekeeping, animal husbandry and breeding. |
| 4 | What levels do | Gate what you may do, raise yields, rare finds and quality, lower failure (burning, misses). Levels never make the world run faster: crops and animals grow at the world's pace. |
| 5 | Where experience comes from | Actions only, whoever performs them. Machines and tamed animals working for you earn nothing; building and configuring them is gated by level. |
| 6 | Hitpoints | Starts at level 10, as in RuneScape. Your maximum health is twice your Hitpoints level: 20 at the start (today's health bar), 198 at 99. |
| 7 | Hunger and warmth | Kept as the Game UI thread built them (`engine::vitals`: hunger 0–20, warmth bands, Well fed and other effects). Food and cooking build on them; see [food-and-cooking.md](food-and-cooking.md). |
| 8 | First content tops out at 50 | The first set of content unlocks up to about level 50, with many things at level 1 (Mariano, 2026-10-06). |
| 9 | Engineering and Crafting | Two more skills (Mariano, 2026-10-06): **Engineering** makes tools, stations and machines and works metal; **Crafting** makes materials, cloth, leather and fine things. Every recipe names the skill it trains (Crafting unless it says), a level it may need, and the experience each one made gives ([tools-and-crafting.md](tools-and-crafting.md)). |

**Hunter.** Breeding lives in Farming, food in Cooking and fighting in Attack, Defence
and Hitpoints, which left wild animals without a skill of their own: tracking, trapping,
catching and taming. Hunter is RuneScape's skill for exactly this, and makes a clean
hand-off: **Hunter gets an animal home, Farming keeps and breeds it.** Mariano chose it
over folding taming into Farming (2026-10-05).

Later skills the systems leave room for, not designed here: Fishing (fish are creatures
already, see [creatures.md](creatures.md)), Woodcutting and Alchemy (every plant already
carries `alchemy:` tags). Smithing is folded into Engineering.

## How a skill works

A skill is content (`skills.ron`): a name, a title and a description. Actions name the
skill they train, the level they need and the experience they give (see
[creature-data.md](creature-data.md)). Levels act through **modifiers** at fixed stages, so
a level bonus and a tool bonus multiply without knowing each other:

| What levels change | Formula | Cap |
| --- | --- | --- |
| Chance of uncommon and rarer finds | `chance × (1 + 0.5 × (level − needed) / (99 − needed))` | ×1.5 at 99; never more than one rarity step |
| Extra common yield | one more with chance `0.5 × (level − needed) / (99 − needed)` | +1 per roll |
| Failure (burning food, a missed catch) | linear from `fail_at_level` to 0 at `safe_level` | per recipe or creature |
| Quality (food, products) | `P(fine) = 0.2 × level / 99 + inputs + station` | data |

These are the crop formulas already in `engine/src/crop.rs` (`level_bonus`, `roll`), made
general in [drops.md](drops.md).

**Experience per action** doubles about every ten levels of what the action needs, so each
level's content trains about as fast as the curve rises (research/farming.md 12.8):
`base_xp ≈ 10 × 2^(needed / 10)`, split across the steps of the work (plant 10%, harvest
70%, check 20% for crops; tame 60%, first product 40% for animals; and so on). Stardew's
`16 × ln(0.018 × value + 1)` fills in for modded content with no hand-set number.

**Combat level** (shown on players and monsters, RuneScape-style):
`combat = (Defence + Hitpoints) / 4 + Attack × 0.65`, rounded down, 3 at the start, 113
at 99/99/99. Monsters are aggressive only to players whose combat level is at most twice
their own (see [creature-ai.md](creature-ai.md), "Aggression").

## The skill guides

Each table lists what the level unlocks in the first set of content. Mariano asked that it
top out around level 50 with plenty at level 1, so a new player starts easily
(2026-10-06); levels 51 to 99 stay open for later content and mods. The guide lines live in
`skills.ron` (`guide:`), and the character screen adds each skill's crops by itself, so a
mod's unlock appears in the guide without editing it. Items in *italics* are supernatural
(see [creatures.md](creatures.md), "Supernatural").

### Attack

Accuracy and damage with weapons. Gates weapons by material; damage rises with level.

| Level | Unlocks |
| --- | --- |
| 1 | Fists, wooden and stone weapons; bone club |
| 3 | Flint spear (reach; good against charging animals) |
| 5 | Copper weapons |
| 10 | Fang dagger (wolf or boar teeth): bleeds |
| 15 | Bronze weapons (copper and tin) |
| 20 | Iron weapons; bows with antler nocks |
| 30 | Cobalt weapons |
| 35 | Silver weapons: +25% against supernatural creatures |
| 40 | Diamond-edged weapons |
| 45 | *Ember-scale blade*: sets targets alight |
| 50 | *Aetherite weapons*: strongest where aether is high |

### Defence

Avoiding and absorbing hits. Gates armour, much of which comes from creatures. It trains
when blows are turned aside.

| Level | Unlocks |
| --- | --- |
| 1 | Hide armour (aurochs, deer, boar) |
| 5 | Hard leather (hide tanned with oak bark) |
| 10 | Copper armour; wolf-pelt cloak (warmth in cold weather) |
| 15 | Bronze armour |
| 20 | Bear-hide armour (warmth, more health); iron armour |
| 25 | Crocodile-scale armour (water) |
| 30 | Cobalt armour |
| 35 | Silver mail: resists supernatural damage |
| 40 | *Ember-scale armour*: fire and heat resistance |
| 45 | *Crystal-shell armour* (crystal tortoise) |
| 50 | *Aetherite armour* |

### Hitpoints

Your health. Your maximum is twice your Hitpoints level (20 at the start, 198 at 99). It
trains alongside Attack whenever you deal damage (a third of the combat experience).

| Level | Unlocks |
| --- | --- |
| 10 | Start: 20 health |
| 20 | 40 health |
| 25 | Healing while Well fed is a quarter faster |
| 35 | Cooked food heals 10% more |
| 50 | 100 health; healing while Well fed is half as fast again |

### Mining

Breaking stone and ore. Levels gate ores and raise ore yields and gem finds; tools set the
speed. Built: `base/tools.ron` gives each ore the Mining level and the pickaxe tier it asks
for and the experience it teaches (natural stone teaches a little; what a player placed
teaches nothing), and the level above the ore's, with the pickaxe's `yield`, raises the
chance of an extra ore.

| Level | Unlocks |
| --- | --- |
| 1 | Stone, dirt, sand, gravel, clay; coal |
| 5 | Copper |
| 10 | Tin; bat guano in caves (fertiliser) |
| 15 | Iron |
| 20 | Salt licks (placed salt draws deer and ibex) |
| 25 | Cobalt |
| 30 | Silver |
| 35 | Gold |
| 40 | *Crystal from a living crystal tortoise's shell* |
| 45 | Diamond |
| 50 | *Aetherite* |

### Farming

Crops, fruit and nut trees, bees, and animal husbandry and breeding. Planting needs the
level; foraging wild plants does not, though their seeds drop only near your level.

| Level | Crops and trees | Animals |
| --- | --- | --- |
| 1 | Wheat | Keep and breed junglefowl and rabbits; collect eggs |
| 3 | Rye, in the cold | Mallards |
| 5 | Carrot; compost (crop scraps and manure) | Mouflon (sheep); shearing |
| 7 | Chickpea, in dry warmth | |
| 8 | | Wild boar (pigs); truffle rooting |
| 10 | Rice, in wet heat with water near (wild rice) | Ibex (goats); milking |
| 11 | Taro, in wet heat (wild taro) | |
| 12 | Potato | Bees: wild hives moved into skeps; honey and wax |
| 15 | | Aurochs (cattle); inspecting shows visible genes |
| 18 | | Alpacas |
| 20 | Cabbage | Horses; the studbook (lineage and inbreeding) |
| 22 | | Reindeer |
| 25 | | Camels, donkeys; mules (horse × donkey) |
| 28 | Tomato | *Moonhares* (rabbits can become them where aether is high) |
| 30 | | *Lumen snails*; inspecting shows hidden genes |
| 32 | Strawberry | Hybrid crosses within a family |
| 35 | | *Ember salamanders* |
| 38 | Chili pepper | Guided mutation: aether feed raises the chance |
| 40 | | *Crystal tortoises* |
| 44 | | *Frost stags* |
| 46 | | *Sunwings* |
| 48 | Saffron | *Griffins* |
| 50 | | *Silver harts* |

### Cooking

Turning ingredients into food. Levels gate recipes and stations and lower the chance of
burning (each recipe sets the level at which you stop burning it).

| Level | Unlocks |
| --- | --- |
| 1 | Campfire: roast meat, fish, eggs, roots and mushrooms; herb tea |
| 2 | Baked potato, roast vegetables |
| 3 | Pot: boiled eggs, porridge; quern: flour, baked into bread |
| 6 | Jam and compote |
| 8 | Animal feed: mash and seed mix (see [breeding.md](breeding.md)) |
| 10 | Oven: bread |
| 12 | Pan: omelette, fried eggs (needs a fat: butter, lard or bear fat) |
| 14 | Vegetable stew |
| 15 | Churn: butter, cream |
| 17 | Meat stew; hunter's stew |
| 19 | Fish soup |
| 21 | Pies: fruit, meat, mushroom |
| 23 | Smoker: smoked meat and fish (the wood sets the flavour) |
| 25 | Fresh cheese |
| 27 | Honey cakes; taming treats |
| 29 | Curry |
| 31 | Drying rack: jerky, dried fruit (needs dry air) |
| 33 | Crock: sauerkraut, pickles, fermented sauces (needs a cool place) |
| 35 | Roast fowl feast; aged cheese |
| 37 | Saffron rice; *glowcap broth*: sight in the dark |
| 39 | Chocolate |
| 41 | *Emberfruit stew*: warmth and fire resistance |
| 43 | *Frostpetal sorbet*: heat resistance |
| 45 | *Moonhare stew*: quiet steps at night; *starlily tea*: see the aether |
| 47 | *Crystal-salt feast*; *sunwing-egg soufflé* |
| 49 | *Heartbloom pie*: +10 maximum health for an hour |
| 50 | Banquet: shared buffs for everyone who eats at the table |

### Hunter

Tracking, trapping and taming wild animals (Mariano chose it as the seventh skill,
2026-10-05). Levels gate which animals you can tame and trap and raise catch rates;
Farming takes over once an animal is yours. Butchering a kill trains it too, and its
level shifts animals' rarer drops.

| Level | Unlocks |
| --- | --- |
| 1 | Tracks: fresh footprints and droppings; tame junglefowl and rabbits; snares |
| 3 | Mallards; bird nets |
| 5 | Mouflon (wild sheep); scent masking with herbs |
| 8 | Ibex; box traps |
| 10 | Wild boar (tamed boars become pigs over generations) |
| 12 | Aurochs; alpacas |
| 16 | Horses; lassos |
| 20 | Wolves (companions, guard flocks); foxes |
| 22 | Reindeer; camels |
| 25 | Lynxes; deer calls and lures |
| 28 | *Moonhares*; reading aether from creature signs |
| 32 | *Ember salamanders* (fire-proof gloves) |
| 35 | Bears (as a mount, after long trust) |
| 40 | *Frost stags* |
| 44 | *Sunwings* |
| 47 | *Griffins* |
| 50 | *Silver harts* |

## Effective level

Every level check (a recipe, an ore, a seed, a catch, a herb planter) sees an effective
level (`engine/src/level.rs`): your level, plus what your effects boost (an effect's
`boosts: [("engineering", 3)]`, at most +6 in all), plus, for a recipe, the best discount a
schematic you carry gives (an item tagged `schematic:<item>:<levels>`, 5 to 10). A recipe
marked `firm` takes neither (the Singularity's Engineering 90). Boosts and schematics help
with making and gathering, never with wielding or wearing. Server and client use the same
functions, so the client predicts what the server allows.

Experience boosts from effects (a good breakfast, Rested, house rooms, feats) multiply
together but stop at a quarter more (`skill::MAX_XP_BOOST`); debuffs still lower it.
