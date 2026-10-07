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
  structure) is looked at once a second where the land is loaded, and kept as it was where
  it is not.

## What there is

| Age | Power | Machines |
| --- | --- | --- |
| Bronze | Rotation: water wheel, windmill, treadwheel (walked by a kept aurochs, horse, camel or bison beside it), shafts, bellows | Trip hammer (plates); the Bellows Bloomery (a multiblock: two iron ore and a fuel into an iron bloom, which a trip hammer beats into three ingots, a crafting table into two) |
| Iron and Steam | Steam: boiler, pipes; beam engine (steam to rotation) | Steam crusher (2 dust an ore) |
| LV | Dynamo (rotation to LV), LV cable, battery bank | Electric furnace, macerator (2 dust, twice as fast), ore washer (3 dust, half as fast, needs water beside it) |
| MV | Steam turbine (steam to MV), MV cable, MV battery bank, LV transformer (MV in on top, LV out of the other faces) | LV machines through a transformer |

## How power moves

Every machine has **ports**: a carrier (rotation, steam or electricity), a tier for
electricity, the faces it connects on, and what it does there: carries, gives, takes, or
holds. Two machines connect where neighbouring ports share a carrier and face each other.
A **network** is everything connected through carrying ports, found by a flood fill when
a machine is placed or broken.

Each factory step (every 4 server ticks) every network shares out its power:

1. Generators say what they can give: their power times their condition (flowing water
   is 1 or 0, wind is the wind's strength times a height factor, fuel is 1 while burning).
2. Consumers ask for what they need: a processor asks its full draw while it has work, a
   converter (beam engine, dynamo) asks for what was used of it a step before.
3. Generators feed consumers first. Stores (battery banks) make up a shortfall or take
   the surplus, up to their rate.
4. Every consumer gets the same share of what it asked. A weak network slows its machines
   rather than stopping one of them.

Fuel burns only as fast as its power is used. A boiler nobody draws on keeps its coal.
Converters give on the next step what they were given on this one, so a chain of them
lags by a step each.

## How items move

Blocks have no facing, so items move by what each machine is. A run of **chutes** (or
**conveyors**, four times as fast) joins the machines beside it; a run moves as many items
a second as its slowest piece (`Chute(rate)` in `machines.ron`).

- A **hopper** gives what players put in it; a processor gives what it made.
- Each item goes, in turn, to a machine on the run that works it or burns it.
- What a processor made that no machine on the run takes goes to a **chest**.
- Bins (hoppers, chests) never feed bins, so a hopper and a chest on one run do nothing.

A furnace fed from a hopper on one run and emptied into a chest on another is the simplest
line: ore and coal in the hopper, ingots in the chest.

## Using a machine

Right click (`base:use_machine`):

- with something in hand that it burns or works, the whole stack goes in;
- with an empty hand (or anything it neither takes nor can place), what it made comes out
  into the inventory;
- with a block it does not take, the block is placed against it.

A hopper or chest takes whatever is in hand, and gives back everything with an empty hand.
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

Machines are saved in the world folder's `machines.ron`, by block and item names, so a
pack that renames or drops a machine loses only that machine.
