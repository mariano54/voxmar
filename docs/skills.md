# Skills

Oct 5, 2026. The skill list and what each level unlocks. Part of the creatures, food and
breeding design: read [systems-web.md](systems-web.md) for how the systems feed each other
and [creature-data.md](creature-data.md) for the data formats.

## Decisions

| # | Question | Decision |
| --- | --- | --- |
| 1 | Curve | RuneScape's, levels 1 to 99 (13,034,431 experience at 99, level 92 is half way), shared by every skill. Already in `core/skills.ron`. |
| 2 | Skills | Attack, Defence, Hitpoints, Mining, Farming, Cooking (Mariano's list), plus **Hunter**, my proposal (see below). |
| 3 | Breeding | Part of Farming, never its own skill (Mariano). Farming covers crops, trees, beekeeping, animal husbandry and breeding. |
| 4 | What levels do | Gate what you may do, raise yields, rare finds and quality, lower failure (burning, misses). Levels never make the world run faster: crops and animals grow at the world's pace. |
| 5 | Where experience comes from | Actions only, whoever performs them. Machines and tamed animals working for you earn nothing; building and configuring them is gated by level. |
| 6 | Hitpoints | Starts at level 10, as in RuneScape. Your maximum health is twice your Hitpoints level: 20 at the start (today's health bar), 198 at 99. |
| 7 | Hunger and warmth | Kept as the Game UI thread built them (`engine::vitals`: hunger 0–20, warmth bands, Well fed and other effects). Food and cooking build on them; see [food-and-cooking.md](food-and-cooking.md). |

**Hunter (my proposal, Mariano to judge).** Mariano asked for a skill per system. Breeding
lives in Farming, food in Cooking and fighting in Attack, Defence and Hitpoints, which
leaves wild animals without a skill of their own: tracking, trapping, catching and taming.
Hunter is RuneScape's skill for exactly this, and it makes a clean hand-off: **Hunter gets
an animal home, Farming keeps and breeds it.** If Mariano prefers fewer skills, taming
moves into Farming unchanged and the Hunter unlocks below become Farming unlocks.

Later skills the systems leave room for, not designed here: Fishing (fish are creatures
already, see [creatures.md](creatures.md)), Woodcutting, Smithing, Crafting (wool, leather,
feathers and bone all have uses waiting), and Alchemy (every plant already carries
`alchemy:` tags).

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

Each table lists what the level unlocks. The Game UI thread draws these as the in-game
skill guides; the data that drives them lives in the content files, so a mod's unlock
appears in the guide by itself. Items in *italics* are supernatural (see
[creatures.md](creatures.md), "Supernatural").

### Attack

Accuracy and damage with weapons. Gates weapons by material; damage rises with level.

| Level | Unlocks |
| --- | --- |
| 1 | Fists, wooden and stone weapons; bone club (from any large animal's bones) |
| 5 | Flint spear (reach; good against charging animals) |
| 10 | Copper weapons |
| 15 | Fang dagger (wolf or crocodile teeth): bleeds |
| 20 | Bronze weapons (copper and tin) |
| 30 | Iron weapons; bows with antler or horn nocks |
| 40 | Cobalt weapons |
| 50 | Silver weapons: +25% against supernatural creatures |
| 60 | Diamond-edged weapons |
| 70 | *Ember-scale blade* (salamander scales): sets targets alight |
| 80 | *Aetherite weapons*: strongest where aether is high |
| 90 | *Griffin-talon spear* |

### Defence

Avoiding and absorbing hits. Gates armour, much of which comes from creatures.

| Level | Unlocks |
| --- | --- |
| 1 | Hide armour (cattle, deer, boar) |
| 10 | Hard leather (hide tanned with oak bark) |
| 15 | Copper armour; wolf-pelt cloak (warmth in cold weather) |
| 20 | Bronze armour |
| 25 | Bear-hide armour (warmth, more health) |
| 30 | Iron armour |
| 35 | Crocodile-scale armour (water) |
| 40 | Cobalt armour |
| 50 | Silver mail: resists supernatural damage |
| 60 | *Ember-scale armour*: fire and heat resistance |
| 70 | *Crystal-shell armour* (crystal tortoise) |
| 80 | *Aetherite armour* |
| 90 | *Griffin-feather mantle*: slow falling |

### Hitpoints

Your health. Your maximum is twice your Hitpoints level (20 at the start, 198 at 99). It
trains alongside Attack or Defence whenever you deal damage (a third of the combat
experience). Damage numbers scale with it: a level-1 monster hits for 1–2, a level-90 one
for up to 40.

| Level | Unlocks |
| --- | --- |
| 10 | Start: 20 health |
| 20 | 40 health |
| 30 | Healing while Well fed is a quarter faster |
| 50 | 100 health; cooked food heals 10% more |
| 70 | Healing while Well fed is half as fast again |
| 99 | 198 health |

### Mining

Breaking stone and ore. Levels gate ores and raise ore yields and gem finds; tools set the
speed.

| Level | Unlocks |
| --- | --- |
| 1 | Stone, dirt, sand, gravel, clay |
| 10 | Coal |
| 15 | Copper |
| 20 | Tin; bat guano in caves (fertiliser) |
| 30 | Iron |
| 40 | Cobalt; salt licks (placed salt draws deer and goats) |
| 50 | Silver |
| 55 | Gold |
| 60 | *Crystal from a crystal tortoise's shell* (with a pickaxe, without hurting it) |
| 70 | Diamond |
| 85 | *Aetherite* |

### Farming

Crops, fruit and nut trees, bees, and animal husbandry and breeding. Planting needs the
level; foraging wild plants does not, though their seeds drop only near your level.

| Level | Crops and trees | Animals |
| --- | --- | --- |
| 1 | Wheat | Keep and breed chickens; collect eggs |
| 5 | Carrot | Rabbits |
| 8 | | Ducks |
| 10 | Compost (crop scraps and manure) | Sheep; shearing |
| 12 | Potato | Pigs; truffle rooting |
| 15 | | Goats; milking |
| 18 | | Bees: wild hives moved into skeps; honey and wax |
| 20 | Cabbage | Cattle; inspecting an animal shows its visible genes |
| 25 | | Alpacas |
| 30 | Tomato | Horses; studbook (track lineage, see inbreeding) |
| 32 | | Reindeer |
| 35 | | Camels; donkeys; mules (horse × donkey) |
| 38 | Strawberry | |
| 40 | | Inspecting shows hidden genes (carriers) |
| 45 | Chili pepper | Hybrid crosses within a family |
| 50 | | *Moonhares* (rabbits can mutate where aether is high) |
| 55 | | *Lumen snails* |
| 60 | Saffron | Guided mutation: aether feed raises mutation chance |
| 62 | | *Ember salamanders* |
| 70 | | *Crystal tortoises* |
| 75 | | *Frost stags* |
| 80 | | *Sunwings* |
| 88 | | *Griffins* |
| 92 | | *Silver harts* |

### Cooking

Turning ingredients into food. Levels gate recipes and stations and lower the chance of
burning (each recipe sets the level at which you stop burning it).

| Level | Unlocks |
| --- | --- |
| 1 | Campfire: roast meat, fish, eggs, roots and mushrooms; herb tea |
| 3 | Baked potato, roast vegetables |
| 5 | Pot: boiled eggs, porridge (oats or millet) |
| 8 | Quern: flour; flatbread |
| 10 | Jam and compote (any fruit or berry) |
| 12 | Animal feed: mash and seed mix (see [breeding.md](breeding.md)) |
| 15 | Oven: bread |
| 18 | Pan: omelette, fried eggs (needs a fat: butter, lard or bear fat) |
| 20 | Vegetable stew |
| 22 | Churn: butter, cream |
| 25 | Meat stew |
| 28 | Fish soup |
| 30 | Pies: fruit, meat, mushroom |
| 33 | Smoker: smoked meat and fish (the wood sets the flavour) |
| 35 | Fresh cheese |
| 38 | Honey cakes; taming treats |
| 40 | Curry (chili and spice) |
| 42 | Drying rack: jerky, dried fruit (needs dry air) |
| 45 | Crock: sauerkraut, pickles, fermented sauces (needs a cool place) |
| 48 | Roast fowl feast |
| 50 | Aged cheese (cellar, weeks) |
| 52 | Saffron rice |
| 55 | *Glowcap broth*: sight in the dark |
| 58 | Chocolate (cacao) |
| 60 | *Emberfruit stew*: warmth and fire resistance |
| 65 | *Frostpetal sorbet*: heat resistance |
| 70 | *Moonhare stew*: quiet steps at night |
| 75 | *Starlily tea*: see the aether |
| 80 | *Crystal-salt feast*: several buffs at once |
| 85 | *Sunwing-egg soufflé* |
| 90 | *Heartbloom pie*: +5 maximum health for an hour |
| 99 | Banquet: shared buffs for everyone who eats at the table |

### Hunter (proposed)

Tracking, trapping and taming wild animals. Levels gate which animals you can tame and trap
and raise catch rates; Farming takes over once an animal is yours.

| Level | Unlocks |
| --- | --- |
| 1 | Tracks: see fresh footprints and droppings; tame chickens and rabbits; snares |
| 5 | Ducks; bird nets |
| 10 | Sheep (wild mouflon); scent masking with herbs |
| 15 | Goats; box traps |
| 20 | Wild boar (tamed boars become pigs over generations) |
| 25 | Cattle (wild aurochs); alpacas |
| 30 | Horses; lassos |
| 35 | Wolves (companions, guard flocks); foxes |
| 40 | Reindeer; camels |
| 45 | Lynxes |
| 50 | *Moonhares*; reading aether from creature signs |
| 55 | Deer calls and lures |
| 60 | *Ember salamanders* (fire-proof gloves) |
| 65 | Bears (as a mount, after years of trust) |
| 72 | *Frost stags* |
| 78 | *Sunwings* |
| 85 | *Griffins* |
| 90 | *Silver harts* |
