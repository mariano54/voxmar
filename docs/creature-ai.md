# Creature AI and behaviour generation

Oct 5, 2026. How creatures sense, decide and move, how each species gets its own way of
behaving, and how behaviours are generated from data. Part of the creatures, food and
breeding design (see [creatures.md](creatures.md)).

Mariano's brief: complex AI that is very different for each creature, unique, interesting
and partly random, shaped by climate and everything else, with a system that generates
behaviours, and room for taming, following, aggro and roaming ranges and distinct ways of
moving. The models and animations stay simple; the behaviour is where the depth goes.

A deep dive into creature AI across the mob mods and other games is running in the
"Minecraft mob mods survey" thread; its findings will be folded in here. This version
stands on the design philosophy (agents choose actions, utility controllers for animals),
on well-known techniques (utility AI, steering behaviours, A* on voxel grids, flow fields)
and on what the mob survey already shows.

## Decisions

| # | Question | Decision |
| --- | --- | --- |
| 1 | Controller | Utility AI: each creature scores the behaviours its species has against its drives, senses and surroundings and runs the best, as the design philosophy's wolf does. No behaviour trees authored per species. |
| 2 | What a behaviour is | A parameterised template from a small engine library (graze, flee, stalk, herd, follow…), composed in data. Mods add templates later through WASM; RON composes and tunes them. |
| 3 | Uniqueness | Four layers: species behaviour set, movement style, regional modulation by climate, biome and aether, and individual personality from a seeded vector. Two foxes from two valleys behave differently, and two foxes from one den differ a little. |
| 4 | Generation | A generator derives a coherent behaviour set from a species' ecology (size, diet, habitat, social structure, activity period, supernatural or not), then perturbs it by seed. Hand-written species override any part. |
| 5 | Determinism | Every random choice draws from `hash(world, creature id, tick, purpose)`. A replay reproduces every creature. |
| 6 | Cost | Decisions at the medium tier (every 4 ticks) near players, the low tier further away, nothing for sleeping creatures until something wakes them; pathfinding budgeted per tick. |

## The loop

```
senses ─▶ drives ─▶ utility selection ─▶ behaviour (phases) ─▶ steering ─▶ locomotion
  ▲                                                                            │
  └──────────────────────────── world, fields, other creatures ◀───────────────┘
```

1. **Senses** turn the world into stimuli: things seen, heard and smelled.
2. **Drives** are a creature's needs, each 0..1, rising and falling over time and with
   events: hunger, thirst, fatigue, fear, aggression, curiosity, social, comfort (too hot,
   too cold, wet), mating (in season), protect (young or nest nearby).
3. **Utility selection** scores each behaviour the species has; the best wins, with
   hysteresis (the running behaviour gets +10%) so creatures don't dither.
4. **Behaviours** are small state machines with phases (stalk: approach → freeze → creep →
   pounce → eat) that set a goal for steering.
5. **Steering** turns goals into velocities: seek, arrive, flee, wander, separation,
   alignment, cohesion, obstacle avoidance, path following.
6. **Locomotion** moves the body with the player physics (`engine/src/physics.rs`) at the
   gait the behaviour asked for.

## Senses

| Sense | Range | What changes it |
| --- | --- | --- |
| Sight | species range in a cone (prey 300°, predators 120°) | light level (night halves it, except nocturnal species), fog and rain (weather), cover (leaves and tall grass block lines of sight), sneaking players count as half as visible |
| Hearing | from each sound's loudness | sound events: sprinting (loud), walking, sneaking (silent), mining and placing (loud), combat, thunder (very loud). Rain and wind mask quiet sounds |
| Smell | species range, stretched downwind | the wind (weather's gust field): ×3 downwind, ×0.3 upwind. Hunters who approach from downwind get close. Blood, food and dung leave scents |
| Vibration | burrowers and wyrms | running feet within range; sneaking is silent |
| Aether | supernatural only | the aether field and aether-bearing items (wisps find wonders) |

Line of sight is a short ray through the voxel grid, budgeted (a few hundred a tick across
all creatures) and cached for a second per pair.

## Drives and considerations

Utility is a product of **considerations**, each a response curve (linear, quadratic,
logistic, step, inverse) over one input. Inputs include the drives, distances (to the
nearest threat, food, water, home, group), the creature's health, the time of day, the
season, the fields at its feet (temperature, moisture, light, rain, aether), the held item
of the nearest player, that player's combat level, and group state (group size, the
leader's behaviour).

Example: graze = hunger (quadratic) × no threat in sight (step) × daylight preference
(this species' activity curve) × food within 8 blocks.

## Behaviour library

Engine templates, each with parameters species set:

| Group | Templates |
| --- | --- |
| Living | wander, graze (eats blocks, which become worn: a grazed meadow shows it), browse (leaves, bushes, fruit), forage (finds items by tag), drink, rest, sleep (in a den, burrow, roost or nest), bask (sun when cold), seek shade (when hot), shelter (from rain and storms), groom, wallow |
| Fear | flee (zigzag, straight or to cover), hide, freeze (rabbits, fawns), burrow, play dead, alarm call, decoy (a duck feigns a broken wing to lead you from her nest), blink (supernatural short teleport) |
| Hunting | stalk, chase, pounce, ambush (in water, in trees, under sand), charge, kite (keep range), pack encircle, scavenge (follows predators and players' kills), steal (eggs, crops, food from chests) |
| Social | herd (boids), follow leader, single file, flock, school, huddle (in cold), play, groom others, court and display (in season), defend young or nest, territory patrol, spar (rutting stags) |
| With players | approach when curious, tempted (follows held food it likes), follow owner, stay, guard owner, fetch, beg, ride, lure (wisps lead you to wonders) |
| Travel | home and return, daily commute (den to feeding ground), migrate (seasonal), follow rivers, roost at dusk |

Mods extend the library by adding templates (WASM, later) and any species can use them.

## Aggression

Whether a creature attacks is decided, not declared:

- **Temperament axes** per species (0..1): aggression, fear, curiosity, sociability,
  territoriality. "Hostile" is high aggression and low fear; "neutral" is low aggression
  until provoked; "passive" never attacks.
- **Aggro radius** = species base × light (nocturnal hunters see further at night) ×
  hunger (hungry wolves come closer in winter) × territory (doubled near a den or nest) ×
  weather (storms make herds jumpy).
- **The RuneScape rule**: a monster is aggressive only to players whose combat level is at
  most twice its own (plus one), so low monsters stop bothering strong players. Neutral
  animals ignore it: a bear guarding cubs attacks anyone.
- **Tolerance**: after about ten minutes spent in one area, aggressive monsters there
  stop attacking you (RuneScape's tolerance), so camping one spot is calm, roaming is not.
- **Provocation and allies**: hitting a neutral creature turns it hostile toward you; its
  group within a call radius joins (wolves, bees, bulls), others flee. Grudges fade over a
  few minutes.
- **Warnings**: many creatures warn before they attack (an adder rattles at 5 blocks and
  strikes at 2; a bear stands up; a goose hisses), so players can back away.

## Roaming

Each creature has a **home** (where it was realised, its den, nest or pen) and a **leash**
radius from its species: rabbit 12 blocks, sheep 24, deer 48, wolf pack territory 96,
bison and reindeer migrate. Beyond the leash, return-home scores high. Tamed animals take
their owner's pen or the owner as home. Daily commutes (bats leave caves at dusk, deer come
to meadows at dawn) are a home and a second feeding ground.

## Following and taming

- **Curious** creatures approach to a comfortable distance and watch; moving fast toward
  them breaks it, sneaking keeps it.
- **Tempted** creatures follow a player holding food they like, as in Minecraft; this is
  how pens get filled.
- **Taming** methods differ by species (see [breeding.md](breeding.md), "Taming"): feeding
  a favourite food until trust fills, patience (stay near without threatening), rescue
  (heal a hurt one), imprinting (be there when an egg hatches), or a gift (a wisp's aether
  mote for a moonhare).
- **Tamed** creatures follow, stay, guard or work. They keep their species' behaviour: a
  tamed wolf still hunts rabbits unless called, and a tamed cat still stalks.

## Movement styles

Each species has a movement style: gaits with speeds and accelerations, turn rate, and
how it moves between them. These are numbers in data, read by one locomotion system, so a
cat and a sheep differ without separate code:

| Style | Parameters that make it |
| --- | --- |
| Cat (lynx, jaguar) | low crouched stalk at 20% speed, freezes whenever its prey looks toward it, explosive pounce (an arc), then a short sprint; turns sharply; pauses often and long |
| Hopper (rabbit, moonhare) | moves in hops of 1–3 blocks with pauses; flees in zigzags; stands up to look |
| Grazer (sheep, aurochs) | slow drift with long pauses, follows neighbours; head down while grazing, up every few seconds to look |
| Bounder (deer) | graceful walk, high bounding run, vigilance checks with ears up; freezes then bolts |
| Trotter (wolf, fox) | steady trot with nose low; packs move in single file on a shared path; circles prey |
| Lumberer (bear) | slow heavy walk, rocks side to side, stands on hind legs to look, short fast charge |
| Slitherer (adder) | sinusoidal path, low, coils to strike |
| Waddler (duck, turtle) | short steps, sways; fast and graceful in water |
| Flier (crow, owl, sunwing) | hops on the ground, flaps up, glides, circles; perches on leaves and logs |
| Schooler (fish) | boids in water; the school turns together |
| Drifter (wisp, lumen snail) | slow, bobbing, unhurried; wisps float |
| Burrower (sand wyrm) | under the surface, a ripple; bursts up |

Animation follows from the same numbers: leg swing from ground speed, a crouch for
stalking, ears and tail from drives.

## Pathfinding

- **Walkable cells.** A creature can stand in a cell if the block below is solid (or water
  for swimmers) and its body height of free space is above. Step up one block, jump up if
  the species can (deer and cats two), drop as far as the species dares (three blocks for
  most, any height for cats and fliers). Avoid lava, fire and cactus; monsters avoid light,
  animals avoid deep water unless they swim.
- **A\*** over those cells with a node budget per search (2,000) and per tick across all
  creatures, so a crowd never costs a frame. Paths are cached and repaired locally when a
  block on them changes.
- **Long routes** (migration, returning home from far) use chunk-level portals: a coarse
  graph of walkable connections between neighbouring columns, built when columns load.
- **Herds** share one path: the leader plans, followers steer to its trail with flocking.
  Large groups heading to one goal (bison stampede) share a flow field.
- **Fliers** plan straight lines with obstacle checks and climb over; swimmers do the same
  in water cells.
- **Getting stuck** is detected (no progress for 2 s) and answered by a random nudge,
  replanning, or giving up the behaviour.

## Climate, biome, weather and time

Behaviour reads the same fields every other system does, so the world shapes it:

| Input | What it changes |
| --- | --- |
| Temperature field | cold: slower, huddle, bask in sun, seek shelter; hot: rest in shade at noon, active at dusk and night (desert foxes are nocturnal, northern ones are not) |
| Moisture and rain | shelter during rain (except ducks and frogs, who come out); smell weakened |
| Storms and thunder | herds spook and may stampede; birds roost; monsters bolder |
| Snow | white-coated individuals bolder (camouflage); tracks show in snow (Hunter) |
| Light and time of day | each species has an activity curve (diurnal, nocturnal, crepuscular); monsters need darkness |
| Season | breeding season (stags rut, aggression up), migration, antler shedding, hibernation (bears), young in spring |
| Aether | supernatural variants and abilities; wisps drift toward wonders |
| Biome tags | which foods and covers exist; cats climb where trees are; wyrms only in sand |
| Players' land | trampled paths, crops (pests come for them), light (keeps monsters away), scarecrows and fences |

## Behaviour generation

Mariano wants a system that generates behaviours, so creatures are varied without
hand-writing each. Four layers, all deterministic from the seed and all data:

### 1. The species behaviour set (generated once per species, rung 2–3)

A generator reads a species' **ecology** and builds a coherent set of behaviours, their
weights and parameters, from rules:

| Ecology | Implies |
| --- | --- |
| Small herbivore | large flee radius, freeze or burrow, hop or dart, nests, many young |
| Large herbivore | herds, vigilance, charge when cornered, migrates if its range is cold or dry |
| Predator, small | stalks and pounces, hunts at dusk, caches or steals |
| Predator, large | chase or ambush by body type (cat: stalk; canid: pack chase; reptile: water ambush) |
| Scavenger | follows predators and players, steals |
| Social: herd, pack, flock, school | the matching group behaviours and alarm calls |
| Activity period | the activity curve; nocturnal species get better night sight |
| Habitat | movement style (climber, swimmer, flier, burrower) and covers it uses |
| Supernatural | one or two behaviours from the supernatural library: blink, phase through leaves, glow pulse, lure, shapeshift by time of day, frost trail, fire aura |

Then **quirks**: 1–3 picked by seed from a library, weighted by ecology, which make a
species memorable: stands up to look, wallows in mud, collects shiny items, plays with
dropped items, sings at dawn, drums the ground in alarm, follows rivers, sunbathes on
rocks, howls in chorus, decoys from the nest, plays dead. Hand-written species can list
their quirks; generated ones get them from the seed.

### 2. The movement style

Picked from body plan and habitat (above), then each number jittered ±15% by the species
seed so no two generated cats move exactly alike.

### 3. Regional modulation (per population cell)

The same species behaves differently by place, read from fields when a cell's population is
first seen: cold populations are diurnal and huddle, hot ones nocturnal; island and remote
populations are bolder and more curious (island tameness); heavily hunted populations
flee earlier (hunting pressure from the population process); high-aether populations get
their supernatural variant's abilities. A rare **learned trait** (foxes here raid farms,
crows here steal from chests) appears in a cell and spreads slowly to neighbouring cells
through the population process, so a region can have a reputation.

### 4. Individual personality

Each individual draws a personality vector from its seed: boldness, curiosity, sociability,
aggression, activity, patience, each a little around its population's values. Genes carry
part of it (temperament is heritable, see [breeding.md](breeding.md)), so breeding calm
animals works. The inspect action shows it in words ("bold, curious, lazy").

### Validation

Every species, generated or written, is simulated in a sandbox before it ships (see the
design philosophy's validation loop): it must eat, drink and rest within a day, flee from a
threat, not get stuck on the test terrains, keep its decision cost under budget, and a
hostile one must reach a player on open ground. Behaviour signatures (time spent per
behaviour, distance travelled, group spread) are recorded so a reskin with identical
behaviour is flagged.

## Simulation cost and networking

- Near players (within 64 blocks) creatures decide every 4 ticks; to 128 blocks every 20;
  beyond, they fold into the regional population (see [creatures.md](creatures.md)).
- Creatures resting with no stimulus sleep: nothing runs until a sound, sight or timer
  wakes them.
- The server sends each creature's position, yaw, gait, and a pose word (crouched,
  grazing, alert, sleeping) when they change, and clients interpolate between updates, as
  they do for other players.
