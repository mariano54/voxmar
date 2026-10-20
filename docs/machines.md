# Machines, power and factories

Phases 6 and 7 of the build plan: the Bronze, Iron and Steam ages' rotation and steam, and
the LV to HV ages' electricity. The code is `crates/engine/src/machine.rs` (definitions),
`crates/engine/src/factory.rs` (networks and work) and `crates/server/src/machines.rs`
(the world, saves and players). The data is `assets/content/base/machines.ron` (what each
machine block does, what burns, which kept animals walk a treadwheel, and how wind and
walkers drive machines), `processing.ron` (what machines make), `multiblocks*.ron` (the
shapes multiblock machines stand in) and `recipes_machines.ron` (how they are built).
Everything a mod might want to change is in that data, not in code.

## The rules it keeps

- **Machines differ in speed and yield, never in output.** A crush recipe makes copper
  dust from copper ore whether a steam crusher or a macerator works it; the macerator
  works twice as fast. `yield` adds outputs: 1.0 doubles them, 0.5 adds one half the time
  (a roll seeded by the machine's position and count made, so it replays the same).
- **Ages gate nothing.** Every machine is a recipe with an Engineering level. Nothing
  checks that an earlier age was reached.
- **Nothing explodes.** Electricity has tiers (1 = LV, 2 = MV, 3 = HV, ...). A machine on a
  network above its tier refuses to run and says it needs a transformer. A cable below the
  network's tier stops the whole network and says which. Players lose time, never blocks.
- **Machines run while their owner is away.** The factory holds every machine whether or
  not its land is loaded. What the world gives a machine (water, wind, a standing
  structure) is looked at every `survey_seconds` (1 by default) where the land is loaded.
  Where it is not, a water wheel keeps what it had, a windmill takes the mean wind
  (`wind_mean`) at its height, and a walked treadwheel stops.
- **What it holds is never made twice (P60).** A save is one generation: machines,
  players, dropped items and land are staged and count together once `world.ron` takes the
  generation, so a save cut short leaves the last whole one and nothing a player put in a
  machine is both in hand and in the machine (see `crates/server/src/save.rs`). A save with
  any part that fails to write never counts. A player leaving brings a save of the whole world (`Server::leave`: buried
  first, then saved as one generation). A machines file that cannot be read is set aside
  as `machines.ron.corrupt-<seconds>` like the other world files (the server will not open
  if it cannot move it) and the server goes on without it; machine blocks standing in saved land join the factory again, empty.
  A machine whose block is gone when its land loads drops what it held, as breaking it
  would. Work half done is saved with it and goes on.

## What there is

| Age | Power | Machines |
| --- | --- | --- |
| Bronze | Rotation: water wheel, windmill, treadwheel (walked by a kept aurochs, horse, camel or bison beside it), shafts, bellows | Trip hammer (plates); the Bellows Bloomery (a multiblock: two iron ore and a fuel into an iron bloom, which a trip hammer beats into three ingots, a crafting table into two) |
| Iron and Steam | Steam: boiler, pipes; beam engine (steam to rotation) | Steam crusher (2 dust an ore); the Coke Oven (a multiblock of kiln bricks three on a side round a hollow middle: coal into coke by its own heat, 30 s); the Blast Furnace (the bloomery's ring four high, on rotation: two iron ore and a coke into four ingots, two an ore, ore only); sawmill (on rotation: a log into six planks of its wood on average, half again an axe's) |
| LV | Dynamo (rotation to LV), LV cable, battery bank | Electric furnace, macerator (2 dust, twice as fast), ore washer (3 dust, half as fast, needs water beside it) |
| MV | Steam turbine (steam to MV), MV cable, MV battery bank, LV transformer (MV in on top, LV out of the other faces) | LV machines through a transformer |
| HV | The Turbine Hall (a multiblock of HV casing five long, three wide and three high, hollow in the middle, its core in the front face: steam to HV, 128 a step from 128 steam: six boilers run it at full, five at 94%), HV cable, HV battery bank, MV transformer (HV in on top, MV out of the other faces) | Processing Array (a multiblock: a cube of HV casing three on a side, hollow in the middle, the array in the middle of one face; sixteen macerators in one, at sixteen times their speed and draw), induction smelter (eight electric furnaces in one); MV and LV machines through transformers |

## How power moves

Every machine has **ports**: a carrier (rotation, steam or electricity), a tier for
electricity, the faces it connects on, and what it does there: carries, gives, takes, or
holds. Two machines connect where neighbouring ports share a carrier and face each other.
A **network** is everything connected through carrying ports, found by a flood fill when
a machine is placed or broken.

Each factory step (every 4 server ticks) every network shares out its power:

1. Generators say what they can give: their power times their condition. A water wheel
   turns on falling or flowing water (1 or 0; still water poured on flat ground does not
   turn it). Each falling or current cell turns one wheel (the first in block order beside
   it), and no other water wheel may stand within `water_spacing` blocks (4) any way, or
   both are still. Wind is the wind's strength times a height factor, and no other windmill may
   stand within `wind_spacing` blocks (4) any way, or both are becalmed. A treadwheel
   needs a kept walker (`walkers`, less any `not_walkers`) within `walk_reach`; its owner
   must own the wheel if the wheel has one, and an animal walks only its nearest wheel.
   What it gives is its working share from the condition rules in creatures.ron
   (`Server::working_share`: 0.5 + 0.7 × condition by default, so 0.5 to 1.2), and nothing
   from a young one or one unfed past `fed.1` days; the condition rules are the only source.
   Fuel is 1 while burning.
2. Consumers ask for what they need: a processor asks its full draw while it has work, a
   converter (beam engine, dynamo) asks for what was used of it a step before.
3. Generators feed consumers first. Stores (battery banks) make up a shortfall or take
   the surplus, up to their rate.
4. Every consumer gets the same share of what it asked. A weak network slows its machines
   rather than stopping one of them.

Every machine carries what is left of a step's progress into its next work, and one fast
enough to finish its work more than once a step does, while it has the inputs and the room,
so a recipe costs the same power at any speed. (Before, a recipe ended its step: an
electric furnace on iron is 4% faster for 4% less power an ingot since, and a weak network
no longer loses part of a step on every item.) HV processors trade
space for speed, never power or yield: a dust costs a processing array what it costs a
macerator. The array crushes about five copper ore a second, more than one conveyor carries.
Arrays built side by side may share a face of casing, as turbine halls may.

Fuel burns only as fast as its power is used. A boiler nobody draws on keeps its coal.
Converters give on the next step what they were given on this one, so a chain of them
lags by a step each.
A converter that is a multiblock's core (the Turbine Hall) neither takes nor gives while its
structure is unfinished or broken; it says its structure is unfinished.

## How items move

Blocks have no facing, so items move by what each machine is. A run of **chutes** (or
**conveyors**, four times as fast) joins the machines beside it; a run moves as many items
a second as its slowest piece (`Chute(rate)` in `machines.ron`).

- A **hopper** gives what players put in it; a processor gives what it made.
- Each item goes, in turn, to a machine on the run that works it or burns it.
- What a processor made that no machine on the run takes goes to a **chest**.
- Bins (hoppers, chests) never feed bins, so a hopper and a chest on one run do nothing.
- A processor has `input_slots` and `output_slots` stacks (3 each by default). One input
  never fills the stacks another recipe input needs, so ore cannot shut out the coal.

A furnace fed from a hopper on one run and emptied into a chest on another is the simplest
line: ore and coal in the hopper, ingots in the chest.

## Using a machine

Right click (`base:use_machine`):

- with something in hand that it burns or works, the whole stack goes in (what has no
  room stays in hand, and nothing comes out);
- with an empty hand (or anything it neither takes nor can place), what it made comes out
  into the inventory;
- with a block it does not take, the block is placed against it.

A hopper takes whatever is in hand but a machine, and gives back everything with an empty
hand. A chest, and every other bin that does not feed (a **store**: barrels, crates,
cupboards, wardrobes, barrel racks, jars, baskets), opens instead (`base:open_store`) into
a screen of its slots in place of the crafting grid, where items move in and out as in the
inventory and shift click sends stacks between it and the pack (`server/src/stores.rs`,
`inventory::STORE`). The store is the bin's: the slots the player sees are a copy kept in
step every tick, so a chute filling it or a second player taking from it shows at once,
and it closes with the screen, out of reach, or when broken (what it held spills). A bin
with `holds` takes only those things, from a hand or a chute: a jar what is `edible`, a
basket what is `forage`. A machine entry naming `{name}` (`{name}_cupboard`) is every
block of that family. A treasure chest whose lock gives opens straight into its slots. A machine with half a recipe (or the wrong things) gives back what went in.
Either way the player is told, in chat, how the machine is doing: working and how far,
no power, no fuel, no wind, no water, no animal walking it, its structure unfinished,
full, too high a voltage, overloaded or idle; a bin says what it holds, a chute how fast
its run moves. `/machine x y z` says the same from anywhere. A
conduit says what its network offers and asks. A machine screen will replace the chat
lines (a `ServerMessage::Machine` carrying `factory::MachineView`, sent while the player
watches it).

## Adding a machine

1. A block in `blocks_machines.ron`, tagged `machine`.
2. Its role in `machines.ron`. A processor needs at least one recipe of its kinds in
   `processing.ron`.
3. A recipe in `recipes_machines.ron` with an Engineering level.
4. Its textures in `assets/packs/default/recipes/machines.ron`, and its Japanese name in
   `base/lang/ja.ron` (with a reading in `readings/ja.ron` if it has kanji).

A multiblock machine's core block must be tagged `multiblock_core` (the registry refuses one
that is not), or it would run whether or not its structure stands.

Every number in `machines.ron` and `processing.ron` is checked as it loads: speeds, power,
rates, capacities and seconds above 0 and finite, yields 0 or more, electricity at a tier of
1 or more. A package with one that is not is refused, with the machine named.

Machines are saved in the world folder's `machines.ron`, by block and item names, so a
pack that renames or drops a machine loses only that machine. A machine remembers who
placed it, and a multiblock counts towards `built:` quests once per standing structure.

Fuels: the last entry in `fuels` naming an item says how long it burns, else the last tag
taking it, so a tag never shortens an item's own entry and a later package can change any of
them.

Fuel heat. Each fuel has a `heat` (1 when unsaid; 0 is refused): logs and planks 1, firewood
and charred wood 2, coal, charcoal and coke 3. A machine or multiblock with a `heat` takes
only fuel at least that hot (`MachineRegistry::burns_in`), so the Crucible Kiln (heat 3)
wants charcoal or coal; a hearth or campfire (heat 0) burns anything.

## The fuel chain (wood to charcoal)

Raw logs are poor fuel. The way up:

1. **Firewood.** A log right clicked on a chopping block with an axe carried (cooking.ron,
   `tool: "axe"`) splits into two firewood, hotter than the log. Any `log`-tagged wood
   splits. The axe's wear is data only (the tool-wear hook adds it).
2. **The charcoal clamp** (multiblocks.ron `charcoal_clamp`): nine firewood three by three under
   soil (earth or turf), kindling in place of the roof's middle earth for its flue. Lit with a
   torch or campfire in hand (`lit_by: "firelighter"`), it smoulders for game hours (`smoulder.hours`), then the firewood is
   charcoal heaps. Rain on its open flue (a bad seal) spoils to ash heaps the share of its
   wood it rained for, from the top down (rain as survival.ron's `douse_rain`). Opened early (a cell broken) it gives part-burnt
   charred wood. Server: `smoulder.rs`, saved in the world's `smoulders`.
3. **Charcoal heat** gates the Crucible Kiln, and so the Farm age's end.

It is the one way to make charcoal (balance call, Oct 20): the earth clamp station, which
charred sixteen logs a minute, and the campfire's log-to-charcoal recipe are gone, so the
effort of processing stays in one station. The charcoal burners' camp still trades twelve
logs for two charcoal, once a player. Charcoal asks for a clamp lit and sixteen
charcoal (or twenty-four coal mined); A Second Clamp, for two lit.
