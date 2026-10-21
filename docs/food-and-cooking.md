# Food and cooking

Oct 6, 2026. Ingredients, recipes, stations, preservation and the Cooking skill, built on
the hunger and warmth the Game UI thread added (`engine::vitals`). Part of the creatures,
food and breeding design (see [creatures.md](creatures.md)).

> **2026-10-06: there is no hunger any more** (Mariano). Food heals: an item heals its
> `heal:N`, a tenth less each time the same food is eaten in a row, down to half,
> recovering as other foods are eaten or a minute passes (`engine::vitals::Recent`).
> Health comes back slowly by itself (a point every ten seconds). Read "hunger" below as
> healing where it still appears.
>
> **2026-10-08: merged wild items** (design plan P27, second round): the wild plants of one
> kind stay in the world as blocks but drop one item, tagged `merged` on the plant:
> `sweet_berry` (sweet berry, blueberry, raspberry, blackberry, cloudberry bushes), `tart_berry`
> (cranberry, lingonberry, gooseberry), `kitchen_herbs` (oregano, parsley, dill, fennel,
> coriander, basil, rosemary, cumin), `wild_roots` (turnip, beet, radish, parsnip), `wild_onion`
> (also leek and garlic, carrying garlic's antidote), `wild_grain` (barley, oats, millet,
> quinoa; tagged grain), `mushroom` (porcini, shiitake, blewit, oyster, chanterelle, brown) and
> `pumpkin` (pumpkin, squash). Each is a placeable plant block that looks like one of its
> plants and carries the `garden` tag (which `regrowth.ron` lists with flower, herb, fungus and berry), so the herb planter takes it and the planted item regrows at its rarity's Farming level; picking a wild plant
> grows that same wild plant back. Kept apart for their own use: mint, sage and thyme (alchemy),
> frost turnip (feeds the frost stag), red mushroom, elderberry, rowan, strawberry, dreamberry,
> glow berries, whisper oats. Old saves' stacks of a merged plant load as the merged item
> (`BlockRegistry::item_named`). `crates/engine/tests/merged_items.rs` checks every plant
> drops its merged item and no recipe, quest or loot names a merged-away plant.
>
> **2026-10-08: how much food heals** (design drafts 55, 63 and 107): cooked food 0.4 × the
> lowest level that cooks it, at least 4 (`vitals::cooked_heal`; about a fifth of the
> health of a player who can make it), so cooking always makes food better; raw produce
> and meat 1, fruit and berries 2; golden apple and emberfruit follow the cooked rule by
> the Farming level that picks them (70 and 41: 28 and 16). Hearty gives a tenth more
> maximum health that comes empty, to fill by eating. `crates/engine/tests/food_heals.rs`
> holds every food to the rule.

## Decisions

| # | Question | Decision |
| --- | --- | --- |
| 1 | Hunger | Kept as built: hunger 0–20 that empties in about 50 minutes, filled by food by tags (`food:N`, or by kind), Well fed heals twice as fast (`engine::vitals`). |
| 2 | Healing | A hybrid. Raw food only fills hunger (Minecraft). Cooked food also heals at once, RuneScape-style, so cooking matters for fighting. |
| 3 | Recipes match properties | A recipe asks for kinds (`meat`, `vegetable`, `herb`), not items. Any creature's meat and any mod's vegetable work in a stew on day one. Named dishes with exact items add flavour on top (the mob survey's main lesson). |
| 4 | A dish is computed | Its hunger, healing, flavour and effects come from its ingredients and technique, with caps, so a venison and carrot stew differs from a carp and leek stew. |
| 5 | Food answers the world | Effects counter the warmth bands that already exist (a warm meal against Chilly and Freezing), and cover the dark, heat, fire and water. |
| 6 | Techniques use fields | Drying needs dry air, fermenting and ageing need a cool place, smoking needs a smoker and wood. The weather's moisture and the temperature field decide where they work. |
| 7 | Cooking skill | Gates recipes and stations, lowers burning, raises quality (see [skills.md](skills.md)). |

## Ingredients

Every edible item is an ingredient through its tags, many of which blocks already carry
(`edible`, `fruit`, `produce`, `root`, `grain`, `berry`, `fungus`, `flavour:*`,
`alchemy:*`, `dye:*`). The food layer adds a few:

| Tag | Meaning |
| --- | --- |
| kind: `meat`, `fish`, `egg`, `dairy`, `fat`, `grain`, `flour`, `vegetable`, `root`, `legume`, `fruit`, `berry`, `nut`, `herb`, `spice`, `fungus`, `sweet` | what recipes match |
| `raw` | cooking it is better; raw meat and fish may give Food poisoning (hunger drains fast for a minute) |
| `flavour:sweet/sour/salty/bitter/savoury/spicy/aromatic/resin` | the seven flavour axes (plus resin from wood) |
| `warming`, `cooling` | shifts how warm you feel after eating (chili, ginger, emberfruit; mint, cucumber, frostpetal) |
| `alchemy:*` | already on plants; some carry into dishes as effects (healing, clarity, night_sight) |
| `aether` | supernatural; strengthens effects and can carry its own |
| `spoils:N` | in-game days until stale; preserved foods have none |
| `food:N` | hunger filled, as vitals reads it today |

Creatures supply: meat (raw chicken, rabbit, duck, mutton, pork, chevon, beef, venison,
camel, crocodile, wyrm), fish (trout, carp, cod, tuna, crab, frog legs), eggs (hen, duck,
turtle, sunwing), dairy (cow, goat, sheep, reindeer and camel milk), fat (lard, bear fat),
honey and wax, and supernatural ingredients (moon dust, lumen slime, ember scale).

Plants supply (the items players hold, as the content defines them in blocks.ron,
crops.ron and trees.ron; since the P27 merge a wild plant of a kind gives one shared item,
named in bold below). Farmed crops: wheat, rye, rice, chickpea, carrot, potato, taro,
cabbage, tomato, strawberry, chili pepper, saffron. Tree fruit and nuts: apple, pear,
plum, cherries, wild cherries, rowan berries, orange, lemon, fig, olives, banana, mango,
avocado, coconut, dates, tamarind, baobab fruit, marula, almond, hazelnut, walnut,
chestnut, pine nuts, cacao, and the golden apple and emberfruit. Wild plants, picked as
items: grains (wild wheat, wild rice, wild corn, **wild grain**: barley, oats, millet,
quinoa), legumes (wild beans, peas, lentils, chickpea), roots (wild carrot, taro, yam,
ginger, cattail, paleroot, **wild roots**: turnip, beet, radish, parsnip), vegetables
(wild asparagus, **wild onion**: onion, leek, garlic; **pumpkin**: pumpkin, squash),
fruit (wild melon, wild pineapple, wild grapes, prickly pear, wild rose hips), berries
(**sweet berry**: sweet berries, blueberry, raspberry, blackberry, cloudberry; **tart
berry**: cranberry, lingonberry, gooseberry; elderberry, currant, goji, juniper), herbs
(**kitchen herbs**: oregano, parsley, dill, fennel, coriander, basil, rosemary, cumin; mint,
sage, thyme, lemongrass, sorrel, nettle, lavender, chamomile, yarrow), spices (wild
vanilla, wild ginger, crocus, juniper), fungi (**mushroom**: porcini, oyster, shiitake,
blewit, chanterelle, brown mushroom; morel, puffball, lion's mane, chicken of the woods,
truffle, inkcap), tea and coffee (tea bush, hibiscus, heather, coffee), and the
fantastical plants (moonbloom, moonmelon, lantern gourd, frost turnip, glowcap, ember
pepper, starwheat, hearthgrain, whisper oats and those proposed in
[systems-web.md](systems-web.md)).

A merged-away plant (tagged `merged` in blocks.ron) stays in the world and drops the
shared item. It is never held, so `BlockRegistry::listed` leaves it out of the creative
catalogue, the journal's Plants page (finding one finds the item) and the recipe book;
`/give` and `/drop` hand out the item, and an older save's stack of one loads as the item
(`item_of`, `item_named`).

## Recipes

A recipe is an action at a station: inputs by kind or item, a technique, a time, a
Cooking level, experience and a burn curve. Its output is a dish whose properties are
computed:

```
hunger   = min(cap, Σ ingredient food × technique factor)      // cooked ×1.5, raw ×1
heal     = hunger × 0.5 × quality                              // instant, only cooked
flavour  = Σ ingredient flavours, scaled; the best-balanced dishes give a little more
effects  = from ingredient tags: warming → Warm meal, cooling → Cool meal,
           alchemy:night_sight → Dark sight, … ; strength from the count and aether
name     = template + main ingredients ("venison and carrot stew")
```

Named dishes (data) match exact ingredients and override the name and add an effect: a
"hunter's stew" (venison, juniper, any root) also gives Quiet steps.

## Stations and techniques

| Station | Technique | Needs | Level |
| --- | --- | --- | --- |
| Campfire | roast | fuel | 1 |
| Pot (on a fire) | porridge and stew (built: the cooking pot, docs/stations.md); later boil, soup, jam | a hob under or beside it, water within 4 | 3 |
| Quern | mill grain to flour (built: quern stones, Crafting 5, four stone fired smooth under a stick; a wheat or rye held and right-clicked grinds into a flour, which bakes at a campfire into bread, half the grain of a grid loaf; design/farm-days.md) | | 3 |
| Oven | bake bread, pies, cakes | fuel | 10 |
| Pan (on a fire) | fry | a fat (butter, lard, bear fat, olive oil) | 12 |
| Churn | butter, cream | milk | 15 |
| Smoker | smoke meat and fish; hours | wood (oak, cherry, maple, juniper, pine each flavour differently) | 23 |
| Cheese press and cellar | fresh then aged cheese; days to weeks | milk, rennet or a herb; a cool place (temperature 4–15 °C) | 25, 35 |
| Drying rack | jerky from raw meat or fish in 10 min (built, docs/stations.md); later dried fruit and herbs | open sky; a fire within 3 doubles the speed; later dry air | 1 (jerky), 31 |
| Crock | ferment: sauerkraut, pickles, sauces, cider; days | a cool place | 33 |
| Table | banquet: dishes set out together, shared effects | | 50 |

The quern has two Farm age companions that are not cooking but work the same way (cooking.ron,
a station tag, two seconds a go, never failing): a **spindle stand** (Crafting 6, with a fired
clay whorl) spins a wool into three twine, and a **loom frame** (Crafting 8, with fired clay
weights) weaves a wool into a plain (white) cloth, or, with a dye flower held and a wool in
the pack, two cloth of the flower's colour, twice what the grid's flower and wool make.

Long techniques (smoking, drying, fermenting, ageing) are due-time events, like crop
stages, computed from the conditions where the station stands and recomputed only when
they change: a drying rack in a desert finishes in an hour, in a swamp it never does, and
rain pauses it.

## Effects

Effects are the vitals effects, plus food buffs, all with a duration and a strength:

| Effect | From | Does |
| --- | --- | --- |
| Warm meal | `warming` ingredients, hot stews and soups | feel up to 8 °C warmer for 10–20 min: counters Chilly and Freezing on a mountain in a blizzard |
| Cool meal | `cooling` ingredients, sorbets | feel up to 8 °C cooler: counters Hot and Overheated in the desert at noon |
| Dark sight | glowcap, lumen slime, carrots in quantity | see in the dark (for caves and night) |
| Fire ward | ember scale, emberfruit | resist fire and lava heat |
| Deep breath | sky lotus, seaweed, fish | breathe longer under water |
| Quiet steps | hunter's stew, moonhare | creatures hear you less (Hunter) |
| Steady hand | honey, tea, coffee | fewer misses (Attack) for a while |
| Hearty | heartbloom, aged cheese | +10 maximum health for an hour |
| Aether sight | starlily tea | see the aether field as a faint shimmer |
| Nourished | a varied diet: five different dishes in the last ten meals | heal faster (stacks with Well fed) |
| Food poisoning | raw meat or fish, spoiled food | hunger drains fast for a minute |

**Variety** matters: eating the same dish again and again fills less each time (back to
full after a few other meals). A varied diet earns Nourished. This pushes players to cook
many things, which pulls them into farming, hunting and foraging.

## Quality and burning

- **Burning.** Each recipe has a level where you start (burning a quarter of the time, or
  the recipe's `burn_start`) and a level where you stop burning it (the mid-teens for the
  first foods: `stop_burning` 15 for meat, fish and potatoes, 12 for eggs and clay), the
  chance falling in a straight line between, as RuneScape. A good station (an oven over a
  campfire) lowers the chance. Burnt food goes to the compost. A cracked clay is clay again
  (`burnt: "clay"` in cooking.ron), so firing it costs time, not the dig.
- **Quality.** Normal, fine, exquisite. The chance of better quality rises with level, the
  quality of the ingredients (a well-kept cow's milk, a crop grown in its comfort) and the
  station. Quality multiplies healing and effect duration.

## Preservation and spoilage

Raw meat, fish, milk and soft fruit go stale after a few in-game days and spoil after a
few more (stale: half the food; spoiled: compost or bait). Cold slows it (the temperature
where the item lies or is carried, so a snowy biome and an ice-lined cellar keep food).
Smoked, dried, salted, pickled and aged foods keep. Spoilage is stamped on the item when
made and read lazily, never ticked; stacks merge only within one freshness step.

This makes food a supply problem for long trips (a mining expedition, a winter journey)
and gives the preserving stations their reason.

## Food for animals and traps

- **Feed** (Cooking 8): mash, seed mix and hay, from grain, roots and greens. Each species
  prefers feed matching its diet; feed is its breeding food and raises its condition (see
  [breeding.md](breeding.md)).
- **Treats** (Cooking 27): honey cakes, fish bits and the like, used for taming by feeding.
- **Bait** (any level): spoiled meat for scavengers and traps, fruit for bats, grain for
  birds, wyrmroot for sand wyrms (Hunter).

## The first dishes

All from real items. Levels are Cooking levels; effects as above.

| Dish | Level | Station | Ingredients | Effect |
| --- | --- | --- | --- | --- |
| Roast meat (any) | 1 | campfire | 1 meat | heals |
| Roast fish (any) | 1 | campfire | 1 fish | heals |
| Roast egg / root / mushroom | 1 | campfire | 1 egg, root or fungus | heals |
| Herb tea | 1 | campfire | 1 herb, water | Steady hand (short) |
| Baked potato | 2 | campfire | potato | heals |
| Boiled eggs | 3 | pot | 2 eggs, water | heals |
| Porridge | 3 | pot | oats or millet, water or milk | Warm meal (short) |
| Flatbread | 5 | campfire | flour, water | heals |
| Jam | 6 | pot | 3 fruit or berries, honey | Steady hand |
| Mash | 8 | pot | grain, root, greens | animal feed |
| Bread | 10 | oven | 2 flour, water | heals |
| Omelette | 12 | pan | 2 eggs, fat, optional herb | heals |
| Vegetable stew | 14 | pot | 2 vegetable or root, herb, water | Warm meal |
| Butter | 15 | churn | milk | (ingredient: fat) |
| Meat stew | 17 | pot | meat, 2 vegetable or root, water | Warm meal |
| Hunter's stew | 17 | pot | venison, juniper, root | Warm meal, Quiet steps |
| Fish soup | 19 | pot | fish, onion or leek, herb | Deep breath |
| Fruit pie | 21 | oven | flour, butter, 2 fruit | heals well |
| Mushroom pie | 21 | oven | flour, butter, 2 fungi | heals well |
| Smoked fish or meat | 23 | smoker | fish or meat | keeps; heals |
| Fresh cheese | 25 | press | milk, sorrel or nettle | Hearty (short) |
| Honey cake | 27 | oven | flour, honey, egg | taming treat; heals |
| Curry | 29 | pot | chili, cumin or coriander, meat or legume, rice | Warm meal (strong) |
| Jerky / dried fruit | 31 | drying rack | meat or fruit | keeps |
| Sauerkraut, pickles | 33 | crock | cabbage or cucumber, salt | keeps; Nourished counts double |
| Roast fowl feast | 35 | oven | whole bird, 2 roots, herb | heals a lot |
| Aged cheese | 35 | cellar | fresh cheese, weeks | Hearty |
| Saffron rice | 37 | pot | rice, saffron, butter | Steady hand (long) |
| Glowcap broth | 37 | pot | glowcap, any fungus, water | Dark sight |
| Chocolate | 39 | pot | cacao, honey or sugar, milk | Steady hand, Warm meal |
| Emberfruit stew | 41 | pot | emberfruit, chili, meat | Fire ward, Warm meal |
| Frostpetal sorbet | 43 | churn | frostpetal, fruit, honey | Cool meal (strong) |
| Moonhare stew | 45 | pot | moonhare, moonbloom, root | Quiet steps (long), Dark sight |
| Starlily tea | 45 | pot | starlily, honey | Aether sight |
| Crystal-salt feast | 47 | table | crystal salt, 3 dishes | their effects together, longer |
| Sunwing-egg soufflé | 47 | oven | sunwing egg, butter, cheese | Warm meal, Hearty |
| Heartbloom pie | 49 | oven | heartbloom, flour, butter, honey | Hearty (strong) |
| Banquet | 50 | table | 5 different dishes | every effect, shared with all who eat |

## Early quest cooking

First Night needs a crafted campfire and successful food cooked at a campfire. Charcoal,
fired clay, ingots and burnt food earn no progress toward its meal, although successful
non-food cooking still trains its skill and counts for quests that ask for that product.
The Crucible Kiln consumes a charge of 32 charcoal or 24 coal when its standing core is
lit. Its paid fire lasts according to `machines.ron` fuel times and is saved across reload;
a cold or dismantled kiln leaves a proposed smelting ingredient untouched.
