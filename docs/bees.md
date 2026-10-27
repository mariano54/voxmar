# Bees

Farm-age bees (Oct 14): wild nests, the skep a swarm goes into, a kept hive that fills with
honey and beeswax, and what the world around a hive does to it. Data in
`assets/content/base/bees.ron` (engine `bees.rs`), blocks in `blocks_bees.ron` (the wild nest
and honey in `blocks.ron`, which generation reads alone), the server's apiary in
`crates/server/src/apiary.rs`.

## The loop

1. **Find a nest.** Wild bees nest in hollows at the foot of broadleaf, fruit and blossom
   trees in flower meadows, cherry groves and lavender fields (`decorations.ron`'s
   `bee_nest`, a tree-foot ornament on biomes tagged `flowers` or `blossom`). Bees drift
   about it. The census (`wild_nest_census`, ignored) found 5 nests in 187 flowering
   columns at chance 0.12; the chance is now 0.2. The quest's hint names every biome the
   decoration grows in, and a test (`the_bee_quest_names_every_biome_wild_bees_nest_in`)
   keeps it so.
2. **Make a skep.** Seven wheat and three twine at a crafting table (Crafting 6).
3. **Take the swarm.** A campfire within four blocks of the nest smokes the bees calm. So
   does smoke on the player: a torch held a moment, or a fire within two blocks, leaves
   them Smoky for 30 seconds (effects.ron's `smoky`, which `lingers`), long enough to put
   the torch away and take up the skep. Hold the skep up to the nest and right-click (`base:tend_bees`,
   Farming 12, 60 Farming XP): the nest is gone and the skep in hand is a skep of bees.
   Without smoke the bees sting (2 damage, never the last health, and Stung, heal ×0.75
   for a minute) and the swarm stays put. Breaking a nest gives one honey and nothing more.
4. **Keep the hive.** Set the skep of bees down. Its bees forage by day (daylight 0.25 or
   more) when it is dry (precipitation 0.1 or less): each look (every ten seconds) adds the
   seconds since the last, times the pace the flowers and crops within six blocks across
   and three up or down give (a honeydew lily 4, any flower 1, a crop 0.5; 12 is full pace).
   Twenty minutes of foraging at full pace fills it: a skep of honey.
5. **Take the honey.** Right-click a full skep: two or three honey and half the time a
   cake of beeswax (45 Farming XP), and the bees stay to fill it again. Without smoke you
   are stung but the honey is yours. Honey heals 2 (as fruit) and stacks (raw:
   `core/stacking.ron`); beeswax and twine make three candles (Crafting 3), against two
   from two hides, so wax light stays a little scarce.

## What the world does to a hive (each a rule in `bees.ron`, each with a test)

| Rule | What happens | Test |
| --- | --- | --- |
| Smoke (`smoke`, `smoke_reach` 4) | a `fire`-tagged block within reach calms the bees: no stings | `a_swarm_goes_into_a_skep_only_under_smoke` |
| Smoke that clings (`smoky`) | a player Smoky (a torch in hand or a fire within 2, lingering 30 s) calms bees as a fire near them would | `smoke_from_a_torch_in_hand_clings_while_the_skep_is_taken_up` |
| Fire (`scorch_reach` 1) | a fire right beside a kept hive drives its bees off: an empty skep | `a_fire_beside_a_hive_drives_its_bees_off` |
| Night and rain (`dusk`, `rain`) | the bees stay in; no foraging | `bees_fill_a_skep_from_the_flowers_near_by_day_only`, `the_games_bees_stay_in_from_rain_and_night_and_follow_the_flowers` |
| Flowers (`nectar`, `nectar_full`) | more flowers, faster honey; none, none | same |
| Crops (`pollinate_reach` 8, `pollinate` 1.25) | crops near a hive with bees grow a quarter faster (`farm::crop_rate`) | `crops_near_a_hive_with_bees_grow_faster` |
| Pets and creatures (`creature_reach` 2.5) | creatures that come close are stung 1 a look, never to death; people (`unstung`) are not | `creatures_by_a_hive_are_stung_and_a_bear_eats_its_honey` |
| Bears (`raiders`) | a brown bear by a full hive eats its honey (the bees stay) | same |
| Shots (`shot_reach` 3) | a sling stone or arrow that lands within 3 blocks of a nest or hive (tagged `bees`), hit or miss, stirs them: unless smoked, they sting the shooter | `a_shot_by_a_bees_nest_stirs_the_bees_unless_smoke_calms_them` (gameplay) |
| Home (`effects.ron` `bee_garden`) | a skep of bees and three flowers at home: heal ×1.1 there, a home step (`Built("home_bee_garden")`) | `a_skep_of_bees_among_flowers_makes_a_bee_garden_at_home` |
| Wanderers (`wandering.ron` `beekeeper`, `home_is: "bee_garden"`) | the beekeeper comes by day to a home with bees: walks the garden (crops within 16 grow a stage, 25 Farming XP) and sells a spare skep for three honey | `the_beekeeper_comes_only_to_a_home_with_bees` |

## Quests (farm)

- **Humming in the Meadow** (farming, after Herb Garden, Food Enough and The Potter): Farming 12, craft a skep, carry
  a skep of bees.
- **A Bee Garden** (home, after it): the bee garden step and three honey; names the Kitchen
  Garden and Herb Garden to do while the skep fills.
- **The Beekeeper Calls** (home, after it): meet the beekeeper and make three candles, or keep
  two beeswax and make six; pays a spare skep.
- **Smoke That Clings** (farming, after Humming in the Meadow): be Smoky (`Felt`) and carry
  four honey; the text says what Smoky calms.
- **Sweet Relief** (cooking, after A Bee Garden): eat three honey (`Ate`) and keep three.

## Saves

`world.ron` keeps `hives`: each kept hive with bees and the seconds it has foraged
(`serde(default)`, so older worlds open with none). A hive in a column nobody has loaded
waits; it is forgotten when its block is no longer a skep with bees.

## Not yet

Villages' skep props are still a hay bale on a log (changing them moves settlement output).
No swarming or splitting of kept colonies; no honey recipes yet (it is an ingredient
waiting for one).
