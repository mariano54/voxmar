# Home stations

Stations that work while you are away (Oct 14, from the home audit's gaps): load one, go
gathering, and come back to finished goods. The first two are the Nomad age's drying rack and
the Farm age's compost heap. Data in `assets/content/base/stations.ron` (engine
`station.rs`, load-checked), blocks in `blocks_stations.ron`, recipes in `recipes_decor.ron`,
the server's side in `crates/server/src/stations.rs`.

## How a station works (general rules, any mod can add one)

- A station is three blocks: `empty`, `working` (loaded) and `ready` (done). Right-click
  (`base:use_station`) with an input in hand loads it: up to `holds` of one input, in whole
  multiples of `takes`; the station asks `level` in the work's skill to load.
- The load is done `minutes` later on the world's clock (`due` is a server tick, saved with
  the world), near or far, loaded column or not. A look every 5 s (`base:stations`, Low tier)
  turns done stations in loaded columns to their ready block; one far away shows it when used.
- Right-click a done station (ready, or still showing working because nobody was near) to take
  `gives` of the output for each `takes` loaded, and `xp` per output in the skill. What does not
  fit pops out beside it.
- `needs` (a spot rule) is asked when it is loaded and again when it is taken: a roof put over a
  rack holds the jerky until the roof comes off. `faster` rules are judged once, as it is
  loaded, and multiply its speed.
- Spot rules: `OpenSky`; `Near(tag, reach)` (a block with the tag within reach, 1 to 8);
  `Water(reach)` (water, not lava, within reach).
- Breaking (or replacing) a loaded station drops what is on it: the raw input before it is
  done, the output after.
- `fertilisers`: an item right-clicked on a growing sown crop (`base:fertilise`) grows the sown
  crops within `reach` by `stages`; with nothing to grow it stays in hand. Quests count the
  crops grown (`Fertilised(item, n)`).

Load checks refuse: a block in two stations or twice in one, `holds` outside 1 to 64, no works,
`takes` above `holds`, `minutes` not above 0, an input that makes itself, an item two works
take, a `Near` tag no block has, a reach outside 1 to 8, a `faster` not above 1 or above 8,
and a fertiliser reach outside 0 to 4.

## The stations

| Station | Age | Recipe | Works | Spot | Time | Skill |
| --- | --- | --- | --- | --- | --- | --- |
| Drying rack | Nomad | 4 sticks, 3 twine (Crafting 3) | up to 8 raw meat or raw fish into jerky, one for one | open sky; a `fire` within 3 doubles the speed | 10 min (5 by a fire) | Cooking 1, 6 xp each |
| Compost heap | Farm | 7 planks in a U (Construction 5) | up to 16 produce, vegetables, fruit, seed or burnt food (`compost` tag), four into one compost | water within 2 makes it x1.5 | 15 min (10 by water) | Farming 5, 8 xp each |

| Fertiliser | Reach | Grows | Xp per crop |
| --- | --- | --- | --- |
| Compost | 1.5 (the crop and its four neighbours) | a stage | Farming 4 |
| Bone | 0.5 (the one crop) | a stage | Farming 2 |

Jerky (new item, tags `meat`, `dried`, `edible`) heals 2 and stacks, like honey and fruit, so it
is trail food; cooked meat heals more but takes a slot each (`core/stacking.ron`). Compost
(new item) is only a fertiliser. A hung or dry rack is tagged `drying`, so it counts as one
of the two bunches a herb nook needs (docs/home.md).

## Tests (each rule, `crates/server/src/stations.rs`)

| Rule | Test |
| --- | --- |
| load, wait, take; xp; the look shows done | `meat_hung_on_a_rack_is_jerky_ten_minutes_later` |
| works while the column is unloaded | `a_rack_left_alone_is_done_when_its_owner_comes_back` |
| open sky at load and at take | `a_rack_dries_only_under_open_sky` |
| a fire near: twice as fast | `a_fire_by_the_rack_dries_twice_as_fast` |
| whole fours, too few, Farming 5, water x1.5 | `a_heap_rots_greens_four_to_one` |
| breaking drops the load (raw or done) | `a_broken_station_drops_what_was_on_it` |
| saves with the world | `loads_keep_across_a_save` |
| compost, bone, nothing to grow; the quest tally | `compost_grows_a_crop_and_its_neighbours` |
| the content's stations and the load checks | `the_games_stations_read_as_their_rules_say`, `a_station_that_makes_no_sense_is_refused` (engine) |

Gate step **Dry** (after Cook): a rack two blocks from the campfire, raw rabbit in hand; a right
click hangs it, the server sends the hung rack back (shot `rack`), and chat says where it
stands makes it quicker.

## Quests

- **Hung to Dry** (Nomad, cooking, after Small Game): craft a drying rack, keep four jerky.
- **Trail Rations** (Nomad, cooking, after it): a second rack, twelve jerky.
- **A Compost Heap** (Farm, farming, after Carrot Patch): craft a compost heap, keep two compost.
- **Rich Soil** (Farm, farming, after it): grow six crops with compost and one with a bone.

## Next

More stations fit the same rules without code: a smoker (meat, Near fire, Cooking 23), a crock
(ferment, a cool spot), dried fruit and herbs on the rack at Cooking 31 (docs/food-and-cooking.md).
A spot rule for dry air (the moisture field, no rain) would let a desert rack dry faster than a
swamp one.
