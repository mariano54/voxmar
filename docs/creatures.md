# Creatures: animals and monsters

Oct 6, 2026. What creatures are, where they live, what they give, and the first bestiary.
Part of the creatures, food and breeding design:

- [creature-ai.md](creature-ai.md): how they behave, and how behaviours are generated
- [breeding.md](breeding.md): taming, husbandry and genetics (Farming)
- [food-and-cooking.md](food-and-cooking.md): ingredients, recipes, Cooking
- [drops.md](drops.md): loot, chances, tools and levels
- [skills.md](skills.md): the skill list and level guides
- [systems-web.md](systems-web.md): how all of it connects
- [creature-data.md](creature-data.md): the data formats and modding

Sources: the mob survey (35 mods, 894 creatures, `research/mob-mods.html`, published at
https://claude.ai/artifact/SLhdhhcRaJyEYQmiSVxubW) and the farming research
(`research/farming.md`). Techniques only; nothing is copied from GPL or ARR mods.

## Decisions

| # | Question | Decision |
| --- | --- | --- |
| 1 | What a creature is | A Definition (`creatures.ron`) for its kind, and an Agent per individual with Traits: body, health, genes, drives, home, owner. One format for animals, monsters, fish and supernatural beings. |
| 2 | Friendly or hostile | Not a type. Temperament axes (aggression, fear, curiosity, sociability, territoriality) plus the situation decide it, so a bear is calm when fed and dangerous near its cubs. |
| 3 | Where they live | A shared pool, like plants: each species states its comfort (temperature, moisture, aether, light, water) and biome selectors; it appears wherever it fits. A mod's creature turns up in every fitting biome without editing a biome. |
| 4 | How many | Rare by default. A regional population process holds numbers per 128-block cell; individuals exist only near players. Hunting lowers numbers, which recover over days. |
| 5 | Supernatural | About 20% of species, tied to the aether field. Rare in ordinary land, the norm in fantasy biomes. Ordinary species also have rare aether variants, and breeding can produce them. |
| 6 | Models | Box models built from a small part library (body, head, legs, tail, wings, horns), coloured by genes; animated procedurally. CC0 or CC-BY packs (Quaternius, Kenney) may replace them later. Never Mojang models. |
| 7 | Server authority | The server simulates creatures and sends position, pose and look; clients interpolate and draw. |

## Kinds of creature

The data has one shape for every creature (see [creature-data.md](creature-data.md)); the
`kind` only sorts them for spawning rules and the bestiary:

| Kind | Spawns | Persistence | Examples |
| --- | --- | --- | --- |
| Animal | from the regional population, near players, day or night as it prefers | wild ones return to the population when far; tamed ones are saved | rabbit, deer, wolf |
| Monster | in darkness (light ≤ 4) or at its lair, near players | never saved; despawn far away | shambler, cave crawler |
| Fish | in water of the right temperature and depth | as animals | trout, tuna |
| Spirit | only where aether is high; no body to butcher | never saved | wisp |

## The body

- **Size and shape.** A box per part, from a part library: torso, head, neck, legs (2, 4, 6
  or 8), tail, ears, horns or antlers, wings, shell, fins. A species picks parts and
  proportions; genes scale and colour them (coat colour, size, horn length).
- **Collision.** One axis-aligned box from the species' size, scaled by the individual's
  size gene, using the player physics already in `engine/src/physics.rs`.
- **Movement.** Gaits (sneak, walk, trot, run, hop, slither, swim, fly, climb) with speeds;
  how they move between them is behaviour (see [creature-ai.md](creature-ai.md)).
- **Animation.** Procedural: legs swing with ground speed, heads turn toward what the
  creature attends to, tails and ears follow the drives (an alarmed rabbit's ears go up).
  No keyframes needed for the first creatures.

## Stats

Every creature has a **level** (its combat level, RuneScape-style) and health, attack,
defence and speed. Wild individuals vary by their genes (±15% around the species), so a
big old boar is tougher than a young one and the inspect action shows it.

## Where creatures live

A species lists:

- **Biome selectors** by tag or name with weights, as biomes list plants
  (`["tag:forest", 1.0], ["base:old_growth_forest", 2.0]`).
- **Comfort**: temperature (°C), moisture, aether, and altitude ranges, the same shape as
  plants.ron. The spawn weight is the selector weight times how well the spot fits, fading
  to zero over a margin, as crops' growth does.
- **Water**: Land, Shore, Shallows, Swim, Deep, Air (fliers still roost on land), Lava.
- **Light and time**: Day, Night, Dusk/Dawn, Any; monsters need darkness.
- **Season**: breeding season, migration (reindeer move to the taiga in winter), hibernation
  (bears are rare in winter).
- **Rarity**: Common 1, Uncommon 0.3, Rare 0.08, VeryRare 0.015, Legendary 0.002, the
  multipliers worldgen already uses (`worldgen/src/rarity.rs`), with each world's seeded
  boosts and suppressions.
- **Group**: size range and structure (lone, pair, family, herd, pack, flock, school,
  swarm).

### Populations: the cheapest update that works

Design principle 12 (cheapest update) and "Simulation level of detail" say a wolf pack
far away is a number. So:

1. **Region cells** of 128 × 128 blocks hold, for each species that fits there, a count and
   a carrying capacity. Capacity = cell area × biome fit × comfort × rarity × the food the
   cell's plants and prey offer. Counts start at capacity × a seeded factor (0.6 to 1.0) the
   first time a cell is looked at; nothing is stored for cells nobody visits.
2. **Near players** (within the streaming radius, at most 96 blocks), the server realises
   individuals from the counts: groups placed on fitting ground, out of sight if possible,
   each with a seeded identity (genes, personality) from `hash(world, cell, species, slot)`.
   A cell realises at most its count; walking away and back shows the same animals.
3. **Far from players**, wild individuals fold back into the count (keeping injuries as a
   lower count). Tamed, named and penned animals are always saved as individuals.
4. **The population process** runs at the very low tier (every 10 s) over visited cells:
   logistic regrowth toward capacity over a few in-game days, predators lowering prey,
   hunting pressure from kills, seasons shifting capacity. Unloaded cells catch up in one
   closed-form step when next visited, as crops do.

Over-hunting a valley therefore empties it for days; wolves thin the deer; a cold winter
moves reindeer south. Nothing is ticked that nobody can see.

### Monsters

Monsters do not come from populations. Near each player, every few seconds, the server
tries a handful of spawn spots in darkness (light ≤ 4, sky or block) and picks a species
by weight × fit, capped per player (8 monsters nearby at most, 3 for a new world's first
night). Aether raises the weight of supernatural monsters. Torches and settled land keep
them away; deep caves and ruins have more. Monsters despawn when no player has been within
64 blocks for 30 s.

## Supernatural

Mariano wants about one creature in five to be supernatural. The hook is the **aether
field** the world already has: a regional noise with a world dial (`dials.fantasy`) that
fantasy biomes require high.

- **Species.** About 20% of species are supernatural (11 of the first 48 below). Their
  comfort includes an aether minimum, so they live only where aether is high: rare finds in
  ordinary land, the norm in the fantasy biomes (moonlit meadow, silverwood glade, glowcap
  forest, ember wastes, crystal expanse, sunspire downs).
- **Encounters.** Tuned so that, averaged over a world at the default dial, about one
  creature met in five is supernatural: about 5% in plain lowlands, 70% in fantasy biomes.
  A validation test measures the share over a large sample of spawn spots (see
  [creature-data.md](creature-data.md), "Validation").
- **Variants.** Ordinary species can have rare aether variants (a gloam wolf is a wolf born
  where aether is high: darker, faster, hostile at night). A variant is a gene, so it can
  be bred for.
- **Breeding.** Mundane parents in a high-aether pen, fed aether plants, can produce a
  supernatural child: rabbits become moonhares (see [breeding.md](breeding.md)).
- **Everything strange does something.** Each supernatural creature drops or produces at
  least one ingredient, material or light source nothing else gives.
- **Plants to match.** The world has 11 fantastical plants among about 170. I proposed 20
  more supernatural plants and crops so plants reach about one in five too; the biome
  thread owns their placement (list in [systems-web.md](systems-web.md), "Plants wanted").

## Products: what living animals give

Living animals produce things on timers, the same due-time heap crops use (no work per
animal per tick): eggs (laid in a nest if one is near), milk (collected with a bucket, once
per cooldown), wool and fleece (sheared; it grows back over days, faster when well fed,
thicker in cold climates), honey and wax (bees, by the flowers within 16 blocks), antlers
(shed once a year, in late winter), manure (fertiliser), truffles (pigs rooting in oak and
chestnut woods), guano (bat roosts). Each product's amount and quality follow the
animal's genes, its condition (fed, watered, sheltered, comfortable) and the keeper's
Farming level. See [breeding.md](breeding.md), "Condition".

## Life

- **Ages**: baby, juvenile, adult, old. Growth is a scheduled event per stage, never a
  tick. Babies follow their mother (or the player they imprinted on, for hatchlings).
- **Lifespan**: wild animals are not simulated to death; kept animals grow old after
  several in-game years (products fall, breeding stops) so studbooks matter.
- **Death**: drops (see [drops.md](drops.md)), and a hunting-pressure mark on the region.

## The first bestiary

48 species, 11 supernatural (*italic*). "Hunter" is the level to tame it, "Farming" the
level to keep and breed it (see [skills.md](skills.md)). The first set tops out at level
50, with the commonest animals at level 1 so a new player starts easily (Mariano,
2026-10-06); creatures' own combat levels stay at 30 and under in the first set (the
brown bear, 28, is the toughest). Diets name real plants and items from plants.ron, crops.ron and
trees.ron.

### Farm and grazing animals

| Species | Lives | Eats | Drops (on death) | Products | Temperament | Hunter / Farming |
| --- | --- | --- | --- | --- | --- | --- |
| Junglefowl (hen) | meadow, plains, forest steppe, jungle edge; 8–32 °C | seeds, wheat, insects | raw chicken, feathers | eggs | skittish, flock | 1 / 1 |
| Rabbit | meadow, plains, steppe, heath, tundra (white coat in snow) | clover, dandelion, carrot, wild carrot | raw rabbit, rabbit hide, rabbit's foot (rare) | manure | skittish, hops | 1 / 1 |
| Mallard | rivers, swamp, bayou, lakes | seeds, water plants, insects | raw duck, feathers, down (shears) | eggs | skittish, swims | 3 / 3 |
| Mouflon (sheep) | meadow, alpine meadow, heath, mountains | grass, clover, heather | mutton, bone | wool (thicker in cold), milk | flock, timid | 5 / 5 |
| Wild boar (pig) | forests with nuts: oak, chestnut, hazel, mixed | hazelnuts, chestnuts, roots, mushrooms | pork, hide, lard, bristles | truffles (rooting), manure | neutral; charges when hurt or near piglets | 10 / 8 |
| Ibex (goat) | mountains, rocky peaks, alpine meadow, mediterranean | heather, juniper, sagebrush, gorse | chevon, hide | milk, horn (shed) | curious, climbs cliffs | 8 / 10 |
| Aurochs (cattle) | plains, forest steppe, meadow; zebu in savanna | grass, clover, wheat, oats | beef, hide, bone | milk, manure | herd; bulls defend | 12 / 15 |
| Alpaca | alpine meadow, mountains, cloud forest | grass, bunchgrass | meat | fleece | spits when annoyed | 12 / 18 |
| Steppe horse | plains, steppe, savanna, forest steppe | grass, oats, apples, carrots | hide | (mount) | herd, flighty | 16 / 20 |
| Reindeer | tundra, snowy taiga; south in winter | lichen, cotton grass, cloudberry | venison, hide | milk, antlers (shed) | herd, migrates | 22 / 22 |
| Donkey | mediterranean, badlands, savanna | dry shrubs, thistle | hide | (pack animal) | stubborn | 16 / 25 |
| Camel | desert, dune sea, outback | saltbush, dead bush, prickly pear | meat, hide | milk | spits, patient | 22 / 25 |
| Bee swarm | flower meadow, lavender fields, meadow, orchards | nectar of flowers within 16 blocks | wax | honey, wax; pollination | neutral; stings if the hive is struck | 1 / 12 |

### Wild animals

| Species | Lives | Eats | Drops | Temperament | Hunter |
| --- | --- | --- | --- | --- | --- |
| Red deer | forests: broadleaf, mixed, maple, old growth, taiga edge | grass, leaves, berries, apples | venison, hide, antler (stags) | skittish; stags spar in autumn rut | 25 (lure) |
| Moose | taiga, bog, northern swamp | willow, water plants | meat, hide, antler | neutral; territorial in rut | — |
| Bison | steppe, plains, sagebrush steppe | grass | meat, hide, horn | herd; stampedes when spooked (thunder) | — |
| Fox | forest, taiga, steppe; arctic coat in snow, fennec ears in desert | rabbits, eggs, berries | fur | crepuscular; pounces; steals eggs | 20 |
| Wolf | taiga, forest, snowy taiga, mountains | deer, sheep, rabbits | pelt, fang | pack; howls at night; bolder when hungry in winter | 20 |
| Lynx | taiga, birch taiga, mountains | rabbits, junglefowl | pelt | shy stalker: freeze, creep, pounce | 25 |
| Brown bear | forest, old growth, taiga, mountains | berries, honey (raids hives), trout, meat | pelt, bear fat, claw | neutral; fierce near cubs; hibernates in winter | 35 |
| Polar bear | glacier, snowy tundra, icy shores | fish | pelt, fat | aggressive when hungry | — |
| Marsh crocodile | swamp, bayou, mangrove swamp, jungle rivers | fish, ducks, whatever comes to drink | scaled hide, meat, tooth | ambushes from water; ignores you far from it | — |
| Adder | heath, desert, badlands, sagebrush | mice, eggs | venom sac, snakeskin | warns at 5 blocks, strikes at 2 | — |
| Jaguar | jungle | anything | pelt, fang | ambusher from trees at night | — |
| Owl | forests at night | rabbits, mice | feathers | perches; hunts at night | — |
| Crow | fields and plains near crops | seeds, ripe crops | feathers | pest flock; scarecrows scare it | — |
| Bat | caves; out at dusk | insects; fruit bats eat figs and mangoes | — | erratic flight; roosts leave guano | — |
| Frog | swamp, bayou, mangrove, rivers | insects, fireflies | frog legs, slime | passive; tongue catches | — |
| Sea turtle | warm beaches | seagrass | scute | lays eggs on beaches; hatchlings head for the sea | — |
| Shore crab | shores, mangrove | detritus | crab meat, shell | pinches if cornered | — |

### Fish

Fish are creatures in water, schooling (see [creature-ai.md](creature-ai.md)). Fishing as a
skill is left for later; nets and traps (Hunter) and spears (Attack) catch them meanwhile.

| Species | Lives | Eats | Drops |
| --- | --- | --- | --- |
| Brown trout | cold rivers and streams | insects | raw trout (weight by size gene) |
| Carp | warm lakes, swamps | plants | raw carp |
| Cod | cold ocean | small fish | raw cod |
| Tuna | warm deep ocean | fish | raw tuna |

### Monsters

| Species | Lives | Drops | Behaviour |
| --- | --- | --- | --- |
| Cave crawler | caves, dark ruins | silk, venom sac | climbs walls, drops from ceilings |
| Bog leech | bog, swamp water | leech (alchemy), slime | latches on, drains slowly |
| Sand wyrm | dune sea, desert | wyrm scale, meat | burrowed; senses running feet, bursts up |
| *Shambler* | darkness where aether is above low; caves | bone, rotten flesh (bait), rare seeds | slow, follows sound, groups at night |

### Supernatural

| Species | Lives (aether high) | Eats | Gives | Behaviour | Hunter / Farming |
| --- | --- | --- | --- | --- | --- |
| *Moonhare* | moonlit meadow; any meadow at night | moonbloom, clover | glowing pelt, moon dust (rare) | blinks a few blocks away when fleeing | 28 / 28 |
| *Lumen snail* | glowcap forest, glowing caves | glowcap, fungi | lumen slime (light, alchemy) | slow; trails glow for a minute | 28 / 30 |
| *Ember salamander* | ember wastes, by lava | emberbloom, emberfruit | ember scale (fuel, fire resistance) | swims in lava; burns what touches it | 32 / 35 |
| *Crystal tortoise* | crystal expanse, crystal caves | crystal bloom | shell crystal (mined from the living animal, Mining 60) | very slow; lives for centuries | 32 / 40 |
| *Frost stag* | crystal expanse, glacier, snowy taiga | frostpetal | frost antler (shed), frost pelt | flees; leaves frost that freezes water | 40 / 44 |
| *Sunwing* | sunspire downs | sunspire seeds, insects | sun feathers (long-burning fuel), egg | circles high at noon, roosts at dusk | 44 / 46 |
| *Griffin* | mountains, rocky peaks, sunspire downs | mouflon, ibex | griffin feather, talon | defends its nest; a mount when tamed | 47 / 48 |
| *Silver hart* | silverwood glade (legendary) | aether orchid, silverwood leaves | shed silver antler, a blessing when befriended | never fights; killing one curses you for a day | 50 / 50 |
| *Wisp* | anywhere aether is high, at night | — | aether mote | drifts toward the nearest wonder, waits for you, fades at dawn | — |
| *Walking mandrake* | where mandrake grows | — | mandrake root | uproots and runs when approached at night; its scream stuns | — |
| *Shambler* (above) | | | | | |

Variants worth having from the start: gloam wolf (wolf, aether), white rabbit and arctic
fox (cold coat gene), fennec fox (desert ears gene), zebu (aurochs, heat gene), albino
(any, rare recessive).

## What comes first

The implementation order is in [creature-data.md](creature-data.md), "Implementation
plan": the entity foundation and data format, a few animals and monsters from the table
above, data-driven AI, drops; then products, taming and breeding; then populations.
